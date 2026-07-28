#[cfg(feature = "desktop")]
pub mod activation;
#[cfg(feature = "desktop")]
pub mod instance;
#[cfg(feature = "desktop")]
pub mod registration;

#[cfg(feature = "desktop")]
pub fn prefers_dark_theme() -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};

    let key = HSTRING::from("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let value = HSTRING::from("AppsUseLightTheme");
    let mut light_theme = 1u32;
    let mut size = std::mem::size_of_val(&light_theme) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut light_theme as *mut u32).cast()),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && light_theme == 0
}
