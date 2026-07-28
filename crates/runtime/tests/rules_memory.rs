#![cfg(any(target_os = "macos", target_os = "linux"))]

use ct_clipboard::ClipboardItem;
use ct_core::{RawRule, RuleEngine};

#[test]
#[ignore = "manual RSS soak probe; run with `just probe-rule-reload-memory`"]
fn repeated_whole_tree_replacement_reports_bounded_rss() {
    let cycles = env_usize("RULE_MEMORY_PROBE_CYCLES", 4);
    let rule_count = env_usize("RULE_MEMORY_PROBE_RULES", 1_024);
    let corpus_size = env_usize("RULE_MEMORY_PROBE_CORPUS", 1_000);
    let allowed_growth_kib = env_u64("RULE_MEMORY_PROBE_ALLOWED_GROWTH_KIB", 32 * 1_024);
    let corpus = make_corpus(corpus_size);
    eprintln!("cycle,stage,rss_kib");
    checkpoint(0, "before-initial-tree");
    let initial_raw = make_tree(0, rule_count);
    checkpoint(0, "initial-raw-built");
    let mut active = RuleEngine::compile(initial_raw).unwrap();
    checkpoint(0, "initial-compiled");
    let mut steady_rss = Vec::with_capacity(cycles);

    for cycle in 0..cycles {
        run_corpus(&mut active, &corpus[..corpus.len().min(128)]);
        checkpoint(cycle, "warm");
        run_corpus(&mut active, &corpus);
        steady_rss.push(checkpoint(cycle, "steady"));

        if cycle + 1 < cycles {
            // Build the complete replacement first. This intentionally covers
            // the brief old/new tree overlap used by desktop hot reload.
            let replacement_raw = make_tree(cycle + 1, rule_count);
            checkpoint(cycle, "replacement-raw-built");
            let replacement = RuleEngine::compile(replacement_raw).unwrap();
            checkpoint(cycle, "replacement-compiled");
            active = replacement;
            checkpoint(cycle, "replacement-installed");
        }
    }

    if let (Some(first), Some(last)) = (steady_rss.first(), steady_rss.last()) {
        assert!(
            last <= &first.saturating_add(allowed_growth_kib),
            "steady RSS grew from {first} KiB to {last} KiB; allowance is \
             {allowed_growth_kib} KiB"
        );
    }
}

fn make_tree(generation: usize, rule_count: usize) -> Vec<RawRule> {
    let mut rules = Vec::with_capacity(rule_count + rule_count / 64);
    for index in 0..rule_count {
        // Unique messages deliberately exercise the pessimistic LinkPure-like
        // case. The compact batch owns each string exactly once.
        rules.push(RawRule {
            kind: Some("url-cleanup".into()),
            id: format!("g{generation}-cleanup-{index}"),
            message: Some(format!("generation {generation}, cleanup {index}")),
            hosts: vec![format!("host{}.example", index % 32)],
            remove_query_params: vec![format!("tracking_{index}")],
            ..RawRule::default()
        });
        if index % 64 == 63 {
            // A mutating rule is an ordering barrier: URL batches on either
            // side must remain separate.
            rules.push(RawRule {
                id: format!("g{generation}-barrier-{index}"),
                from: Some("never-matches-this-corpus".into()),
                to: Some("replacement".into()),
                ..RawRule::default()
            });
        }
    }
    rules
}

fn make_corpus(size: usize) -> Vec<ClipboardItem> {
    (0..size)
        .map(|index| {
            ClipboardItem::from_text(format!(
                "https://host{}.example/path?keep={index}&tracking_{}={index}",
                index % 32,
                index % 1_024
            ))
        })
        .collect()
}

fn run_corpus(engine: &mut RuleEngine, corpus: &[ClipboardItem]) {
    for item in corpus {
        let _ = engine.try_apply(item).unwrap();
    }
}

fn checkpoint(cycle: usize, stage: &str) -> u64 {
    let rss = current_rss_kib();
    eprintln!("{cycle},{stage},{rss}");
    rss
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn current_rss_kib() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
        // SAFETY: proc_pidinfo writes at most the supplied proc_taskinfo size
        // into a valid, suitably aligned output pointer for this process.
        let written = unsafe {
            libc::proc_pidinfo(
                std::process::id() as libc::c_int,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int,
            )
        };
        assert_eq!(written as usize, std::mem::size_of::<libc::proc_taskinfo>());
        // SAFETY: the size check above proves proc_pidinfo initialized it.
        unsafe { info.assume_init() }.pti_resident_size / 1024
    }

    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string("/proc/self/statm").expect("read /proc/self/statm");
        let resident_pages = statm
            .split_whitespace()
            .nth(1)
            .expect("statm resident pages")
            .parse::<u64>()
            .expect("statm resident pages are numeric");
        // SAFETY: _SC_PAGESIZE is a read-only process query.
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        assert!(page_size > 0);
        resident_pages.saturating_mul(page_size as u64) / 1024
    }
}
