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
