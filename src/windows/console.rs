use anyhow::Result;
use windows::Win32::{Foundation::HANDLE, System::Console::*};
pub struct RawConsole {
    input: HANDLE,
    output: HANDLE,
    input_mode: CONSOLE_MODE,
    output_mode: CONSOLE_MODE,
}
impl RawConsole {
    pub fn enter() -> Result<Self> {
        unsafe {
            let input = GetStdHandle(STD_INPUT_HANDLE)?;
            let output = GetStdHandle(STD_OUTPUT_HANDLE)?;
            let mut input_mode = CONSOLE_MODE(0);
            let mut output_mode = CONSOLE_MODE(0);
            GetConsoleMode(input, &mut input_mode)?;
            GetConsoleMode(output, &mut output_mode)?;
            let state = Self {
                input,
                output,
                input_mode,
                output_mode,
            };
            let mode = (input_mode | ENABLE_VIRTUAL_TERMINAL_INPUT | ENABLE_EXTENDED_FLAGS)
                & !(ENABLE_ECHO_INPUT
                    | ENABLE_LINE_INPUT
                    | ENABLE_PROCESSED_INPUT
                    | ENABLE_QUICK_EDIT_MODE);
            SetConsoleMode(input, mode)?;
            SetConsoleMode(
                output,
                output_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT,
            )?;
            Ok(state)
        }
    }
}
impl Drop for RawConsole {
    fn drop(&mut self) {
        unsafe {
            let _ = SetConsoleMode(self.input, self.input_mode);
            let _ = SetConsoleMode(self.output, self.output_mode);
        }
    }
}
pub fn size() -> (u16, u16) {
    unsafe {
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        if let Ok(h) = GetStdHandle(STD_OUTPUT_HANDLE) {
            if GetConsoleScreenBufferInfo(h, &mut info).is_ok() {
                return (
                    (info.srWindow.Right - info.srWindow.Left + 1).max(1) as u16,
                    (info.srWindow.Bottom - info.srWindow.Top + 1).max(1) as u16,
                );
            }
        }
    }
    (80, 24)
}
pub fn title() -> String {
    let mut buf = vec![0; 32768];
    let n = unsafe { GetConsoleTitleW(&mut buf) };
    String::from_utf16_lossy(&buf[..n as usize])
}
pub fn set_title(title: &str) -> Result<()> {
    let mut text = title.encode_utf16().collect::<Vec<_>>();
    text.push(0);
    unsafe {
        SetConsoleTitleW(windows::core::PCWSTR(text.as_ptr()))?;
    }
    Ok(())
}
