#![cfg(target_os = "linux")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use ct_clipboard::native::X11ClipboardBackend;
use ct_clipboard::{ClipboardBackend, ClipboardItem, ClipboardPlatform, NativeRepresentation};

#[test]
fn x11_backend_observes_and_transfers_multiple_formats() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("skipping X11 runtime test without DISPLAY");
        return;
    }

    let done = Arc::new(AtomicBool::new(false));
    let writer_done = Arc::clone(&done);
    let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
    let writer = std::thread::spawn(move || {
        let mut backend = X11ClipboardBackend::new().unwrap();
        let mut item = ClipboardItem::for_platform(ClipboardPlatform::X11);
        item.set_text("hello from X11");
        item.set_html("<b>hello from X11</b>");
        item.set_native(NativeRepresentation::x11(
            "application/x-test-bytes",
            "application/x-test-bytes",
            8,
            vec![0, 1, 2, 3],
            false,
        ));
        item.set_native(NativeRepresentation::x11(
            "application/x-test-incr",
            "application/x-test-incr",
            8,
            vec![9; 20 * 1024 * 1024],
            false,
        ));
        backend.write(&item).unwrap();
        ready_sender.send(()).unwrap();
        while !writer_done.load(Ordering::Relaxed) {
            backend.change_count().unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
    });

    ready_receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut reader = X11ClipboardBackend::new().unwrap();
    assert!(reader.read_limited(1024 * 1024).unwrap().is_none());
    let item = reader.read_limited(32 * 1024 * 1024).unwrap().unwrap();
    done.store(true, Ordering::Relaxed);
    writer.join().unwrap();

    assert_eq!(item.text(), Some("hello from X11"));
    assert_eq!(item.html(), Some("<b>hello from X11</b>"));
    assert_eq!(
        item.representation("application/x-test-bytes")
            .map(NativeRepresentation::data),
        Some([0, 1, 2, 3].as_slice())
    );
    assert_eq!(
        item.representation("application/x-test-incr")
            .map(NativeRepresentation::data)
            .map(<[u8]>::len),
        Some(20 * 1024 * 1024)
    );
    assert!(item
        .representation("application/x-test-incr")
        .unwrap()
        .flags()
        .contains(&ct_clipboard::NativeFormatFlag::Incremental));
}
