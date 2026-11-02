//! 사람이 읽는 출력의 언어. Claude가 읽는 텍스트와 `--help`는 영어로 고정한다. (FR-UI-1, FR-UI-2)

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    Ko,
}

/// `tr!(lang, "english {x}", "한국어 {x}", args…)`: 언어에 맞는 형식 문자열로 `format!`한다.
#[macro_export]
macro_rules! tr {
    ($lang:expr, $en:literal, $ko:literal $(, $arg:expr)* $(,)?) => {
        match $lang {
            $crate::i18n::Lang::En => format!($en $(, $arg)*),
            $crate::i18n::Lang::Ko => format!($ko $(, $arg)*),
        }
    };
}

/// 유닉스 초를 로컬 시간대의 `YYYY-MM-DD HH:MM:SS`로 바꾼다. 문자열 순서가 시간 순서와 같다. (FR-PRJ-6)
pub fn local_time(secs: i64) -> String {
    let time = secs as libc::time_t;
    // SAFETY: `localtime_r`는 넘겨준 두 포인터만 읽고 쓰며, 둘 다 이 함수 안에서 유효하다.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&time, &mut tm).is_null() };
    if !ok {
        return secs.to_string();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}
