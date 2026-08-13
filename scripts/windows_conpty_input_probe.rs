use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::Write as IoWrite;
use std::path::Path;

const STD_INPUT_HANDLE: u32 = -10i32 as u32;
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
const ENABLE_LINE_INPUT: u32 = 0x0002;
const ENABLE_ECHO_INPUT: u32 = 0x0004;
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
const KEY_EVENT: u16 = 0x0001;

type Handle = *mut c_void;

#[link(name = "Kernel32")]
unsafe extern "system" {
    fn GetStdHandle(kind: u32) -> Handle;
    fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
    fn SetConsoleMode(handle: Handle, mode: u32) -> i32;
    fn ReadFile(
        handle: Handle,
        buffer: *mut c_void,
        bytes_to_read: u32,
        bytes_read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn ReadConsoleInputW(
        handle: Handle,
        records: *mut InputRecord,
        length: u32,
        records_read: *mut u32,
    ) -> i32;
    fn WriteFile(
        handle: Handle,
        buffer: *const c_void,
        bytes_to_write: u32,
        bytes_written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KeyEventRecord {
    key_down: i32,
    repeat_count: u16,
    virtual_key_code: u16,
    virtual_scan_code: u16,
    unicode_char: u16,
    control_key_state: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
union InputRecordEvent {
    key: KeyEventRecord,
}

#[repr(C)]
struct InputRecord {
    event_type: u16,
    event: InputRecordEvent,
}

fn append_report(report: &mut File, line: &str) {
    if writeln!(report, "{line}")
        .and_then(|_| report.flush())
        .is_err()
    {
        std::process::exit(4);
    }
}

fn write_all(handle: Handle, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let mut written = 0;
        let ok = unsafe {
            WriteFile(
                handle,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || written == 0 {
            std::process::exit(3);
        }
        bytes = &bytes[written as usize..];
    }
}

fn open_report(path: &Path) -> File {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap_or_else(|_| std::process::exit(4))
}

fn empty_record() -> InputRecord {
    InputRecord {
        event_type: 0,
        event: InputRecordEvent {
            key: KeyEventRecord {
                key_down: 0,
                repeat_count: 0,
                virtual_key_code: 0,
                virtual_scan_code: 0,
                unicode_char: 0,
                control_key_state: 0,
            },
        },
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().unwrap_or_else(|| "legacy".to_string());
    let report_path = arguments
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::process::exit(64));
    if arguments.next().is_some() || !matches!(mode.as_str(), "legacy" | "kitty" | "native") {
        std::process::exit(64);
    }

    let mut report = open_report(&report_path);
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let mut console_mode = 0;
    if unsafe { GetConsoleMode(input, &mut console_mode) } == 0 {
        append_report(&mut report, "ERROR:get-console-mode");
        std::process::exit(1);
    }
    if mode != "native" {
        let raw_vt_mode = (console_mode | ENABLE_VIRTUAL_TERMINAL_INPUT)
            & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
        if unsafe { SetConsoleMode(input, raw_vt_mode) } == 0 {
            append_report(&mut report, "ERROR:set-console-mode");
            std::process::exit(2);
        }
    }

    if mode == "kitty" {
        write_all(output, b"\x1b[>7u\x1b[?u\x1b[c");
    }
    append_report(&mut report, &format!("READY:{}", mode.to_ascii_uppercase()));

    if mode == "native" {
        let mut records: [InputRecord; 16] = std::array::from_fn(|_| empty_record());
        loop {
            let mut read = 0;
            let ok = unsafe {
                ReadConsoleInputW(input, records.as_mut_ptr(), records.len() as u32, &mut read)
            };
            if ok == 0 || read == 0 {
                break;
            }
            for record in &records[..read as usize] {
                if record.event_type != KEY_EVENT {
                    continue;
                }
                let key = unsafe { &record.event.key };
                append_report(
                    &mut report,
                    &format!(
                        "RECORD:{};{};{};{};{};{}",
                        if key.key_down != 0 { 1 } else { 0 },
                        key.repeat_count,
                        key.virtual_key_code,
                        key.virtual_scan_code,
                        key.unicode_char,
                        key.control_key_state,
                    ),
                );
            }
        }
    } else {
        let mut all = Vec::new();
        let mut buffer = [0u8; 256];
        loop {
            let mut read = 0;
            let ok = unsafe {
                ReadFile(
                    input,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || read == 0 {
                break;
            }
            all.extend_from_slice(&buffer[..read as usize]);
            let hex = all
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            append_report(&mut report, &format!("HEX:{hex}"));
        }
    }
}
