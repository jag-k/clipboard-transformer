#![cfg(target_os = "linux")]

use ct_clipboard::native::{
    probe_clipboard_backend, LinuxClipboardBackend, LinuxClipboardBackendKind,
};
use ct_clipboard::{ClipboardBackend, ClipboardItem, ClipboardPlatform, NativeRepresentation};

#[test]
fn wayland_data_control_observes_and_transfers_multiple_formats() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("skipping Wayland runtime test without WAYLAND_DISPLAY");
        return;
    }

    assert!(matches!(
        probe_clipboard_backend().unwrap(),
        Some(
            LinuxClipboardBackendKind::WaylandExtDataControl
                | LinuxClipboardBackendKind::WaylandWlrDataControl
        )
    ));

    let mut writer = LinuxClipboardBackend::new().unwrap();
    let mut item = ClipboardItem::for_platform(ClipboardPlatform::Wayland);
    item.set_text("hello from Wayland");
    item.set_html("<b>hello from Wayland</b>");
    item.set_native(NativeRepresentation::named(
        "application/x-test-bytes",
        vec![4, 5, 6, 7],
    ));
    writer.write(&item).unwrap();

    let mut reader = LinuxClipboardBackend::new().unwrap();
    let item = reader.read_limited(1024 * 1024).unwrap().unwrap();

    assert_eq!(item.text(), Some("hello from Wayland"));
    assert_eq!(item.html(), Some("<b>hello from Wayland</b>"));
    assert_eq!(
        item.representation("application/x-test-bytes")
            .map(NativeRepresentation::data),
        Some([4, 5, 6, 7].as_slice())
    );
}
