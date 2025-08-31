#![macro_use]

use std::cmp::{min};
use std::collections::{HashMap, HashSet};
use std::fmt::{Display};
use std::io::{Error};
use std::path::{PathBuf};
use std::sync::Mutex;

use colored::Colorize;
use once_cell::sync::Lazy;
use peg::error::ParseError;
use peg::str::LineCol;

use crate::preprocess::*;

struct WarningState {
    max: u32,
    muted: HashSet<String>,
    raised: HashMap<String, u32>,
}

static WARNING_STATE: Lazy<Mutex<WarningState>> = Lazy::new(|| {
    Mutex::new(WarningState {
        max: 10,
        muted: HashSet::new(),
        raised: HashMap::new(),
    })
});

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => (
        std::io::Error::new(std::io::ErrorKind::Other, format!($($arg)*))
    )
}

pub trait ErrorExt<T> {
    fn prepend_error<M: AsRef<[u8]> + Display>(self, msg: M) -> Result<T, Error>;
    fn print_error(self, exit: bool) -> ();
}
impl<T> ErrorExt<T> for Result<T, Error> {
    fn prepend_error<M: AsRef<[u8]> + Display>(self, msg: M) -> Result<T, Error> {
        match self {
            Ok(t) => Ok(t),
            Err(e) => Err(error!("{}\n{}", msg, e))
        }
    }

    fn print_error(self, exit: bool) {
        if let Err(error) = self {
            eprintln!("{}: {}", "error".red().bold(), error);

            if exit {
                print_warning_summary();
                std::process::exit(1);
            }
        }
    }
}

pub trait PreprocessParseErrorExt<T> {
    fn format_error(self, origin: &Option<PathBuf>, input: &str) -> Result<T, Error>;
}
impl<T> PreprocessParseErrorExt<T> for Result<T, ParseError<LineCol>> {
    fn format_error(self, origin: &Option<PathBuf>, input: &str) -> Result<T, Error> {
        match self {
            Ok(t) => Ok(t),
            Err(pe) => {
                let line_origin = pe.location.line - 1;
                let file_origin = match origin {
                    Some(ref path) => format!("{}:", path.to_str().unwrap().to_string()),
                    None => "".to_string()
                };

                let line = input.lines().nth(pe.location.line - 1).unwrap_or("");

                Err(format_parse_error(line, file_origin, line_origin, pe.location.column, &pe.expected))
            }
        }
    }
}

pub trait ConfigParseErrorExt<T> {
    fn format_error(self, info: &PreprocessInfo, input: &str) -> Result<T, Error>;
}
impl<T> ConfigParseErrorExt<T> for Result<T, ParseError<LineCol>> {
    fn format_error(self, info: &PreprocessInfo, input: &str) -> Result<T, Error> {
        match self {
            Ok(t) => Ok(t),
            Err(pe) => {
                let line_origin = info.line_origins[min(pe.location.line, info.line_origins.len()) - 1].0 as usize;
                let file_origin = match info.line_origins[min(pe.location.line, info.line_origins.len()) - 1].1 {
                    Some(ref path) => format!("{}:", path.to_str().unwrap().to_string()),
                    None => "".to_string()
                };

                let line = input.lines().nth(pe.location.line - 1).unwrap_or("");

                Err(format_parse_error(line, file_origin, line_origin, pe.location.column, &pe.expected))
            }
        }
    }
}

fn format_parse_error(line: &str, file: String, line_number: usize, column_number: usize, expected: &impl Display) -> Error {
    let trimmed = line.trim_start();

    error!("In line {}{}:\n\n  {}\n  {}{}\n\nUnexpected token \"{}\", expected: {}",
        file,
        line_number,
        trimmed,
        " ".to_string().repeat(column_number - 1 - (line.len() - trimmed.len())),
        "^".red().bold(),
        line.chars().map(|x| x.to_string()).nth(column_number - 1).unwrap_or_else(|| "\\n".to_string()),
        expected)
}

fn print_warning_message<M: AsRef<[u8]> + Display>(msg: M, name: Option<&'static str>, location: (Option<M>,Option<u32>)) {
    let loc_str = if location.0.is_some() && location.1.is_some() {
        format!("In file {}:{}: ", location.0.unwrap(), location.1.unwrap())
    } else if location.0.is_some() {
        format!("In file {}: ", location.0.unwrap())
    } else if location.1.is_some() {
        format!("In line {}: ", location.1.unwrap())
    } else {
        "".to_string()
    };

    let name_str = match name {
        Some(name) => format!(" [{}]", name),
        None => "".to_string()
    };

    eprintln!("{}{}: {}{}", loc_str, "warning".yellow().bold(), msg, name_str);
}

pub fn warning<M: AsRef<[u8]> + Display>(msg: M, name: Option<&'static str>, location: (Option<M>,Option<u32>)) {
    let mut state = WARNING_STATE.lock().unwrap();

    if let Some(name_str) = name {
        if state.muted.contains(name_str) {
            return;
        }

        let max_warnings = state.max;
        let raised_count = state.raised.entry(name_str.to_string()).or_insert(0);
        if *raised_count >= max_warnings {
            return;
        }
        *raised_count += 1;
    }

    // Drop the lock before printing to avoid deadlocks if printing logic ever changes to call back into this module.
    drop(state);
    print_warning_message(msg, name, location);
}

pub fn warning_suppressed(name: Option<&'static str>) -> bool {
    let name = match name {
        Some(n) => n,
        None => return false,
    };

    let state = WARNING_STATE.lock().unwrap();

    if state.muted.contains(name) {
        return true;
    }

    if let Some(raised) = state.raised.get(name) {
        raised >= &state.max
    } else {
        return false;
    }
}

pub fn print_warning_summary() {
    let state = WARNING_STATE.lock().unwrap();
    let mut summary_warnings = Vec::new();

    for (name, raised) in state.raised.iter() {
        if state.muted.contains(name) { continue; }

        if *raised > state.max {
            let excess = *raised - state.max;
            let msg = if excess > 1 {
                format!("{} warnings of type \"{}\" were suppressed to prevent spam. Use \"-w {}\" to disable these warnings entirely.", excess, name, name)
            } else {
                format!("{} warning of type \"{}\" was suppressed to prevent spam. Use \"-w {}\" to disable these warnings entirely.", excess, name, name)
            };
            summary_warnings.push(msg);
        }
    }

    drop(state);

    for msg in summary_warnings {
        print_warning_message(msg, None, (None, None));
    }
}

pub fn init_warnings(muted: HashSet<String>, verbose: bool) {
    let mut state = WARNING_STATE.lock().unwrap();
    state.muted = muted;
    if verbose {
        state.max = u32::MAX;
    }
}
