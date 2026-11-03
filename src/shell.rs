//! Bash 명령에서 실제로 실행되는 명령이 hippo인지 판정한다. (FR-STO-11)

use std::collections::HashMap;
use std::io::BufReader;
use std::path::Path;

use brush_parser::ast::{
    AndOr, AndOrList, AssignmentName, AssignmentValue, Command, CommandPrefixOrSuffixItem,
    CompoundCommand, CompoundList, CompoundListItem, Pipeline, SimpleCommand, Word,
};
use brush_parser::word::{Parameter, ParameterExpr, WordPiece, WordPieceWithSource};
use brush_parser::{Parser, ParserOptions};

const HIPPO_EXE: &str = "hippo";
/// 뒤에 오는 명령을 실행하는 래퍼. 실행 단어를 찾을 때 건너뛴다.
const WRAPPERS: &[&str] = &["env", "time", "sudo", "nohup", "exec", "command"];
/// 인자로 받은 `이름=값`을 변수로 정하는 내장 명령.
const DECLARERS: &[&str] = &["export", "declare", "typeset", "local", "readonly"];
/// 확장을 알 수 없는 자리. 파일 이름 판정에서 이 문자가 들어간 부분은 쓰지 않는다.
const UNKNOWN: char = '\u{0}';

/// 명령 줄에서 hippo가 하나라도 실행되면 true. 파싱에 실패하면 첫 단어 규칙으로 판정한다.
pub fn runs_hippo(command: &str) -> bool {
    let options = ParserOptions::default();
    let mut parser = Parser::new(BufReader::new(command.as_bytes()), &options);
    match parser.parse_program() {
        Ok(program) => {
            let mut walker = Walker {
                options,
                vars: HashMap::new(),
                found: false,
            };
            for list in &program.complete_commands {
                walker.list(list);
            }
            walker.found
        }
        Err(_) => first_word_is_hippo(command),
    }
}

/// 따옴표를 벗긴 첫 단어의 파일 이름이 hippo인가.
fn first_word_is_hippo(command: &str) -> bool {
    let command = command.trim_start();
    let first = match command.chars().next() {
        Some(q @ ('\'' | '"')) => command[1..].split(q).next().unwrap_or(""),
        _ => command.split_whitespace().next().unwrap_or(""),
    };
    is_hippo_name(first)
}

fn is_hippo_name(path: &str) -> bool {
    Path::new(path).file_name().is_some_and(|n| n == HIPPO_EXE)
}

struct Walker {
    options: ParserOptions,
    /// 변수 이름 → 값이 hippo 실행 파일을 가리키는가.
    vars: HashMap<String, bool>,
    found: bool,
}

impl Walker {
    fn list(&mut self, list: &CompoundList) {
        for CompoundListItem(and_or, _) in &list.0 {
            self.and_or(and_or);
        }
    }

    fn and_or(&mut self, list: &AndOrList) {
        self.pipeline(&list.first);
        for next in &list.additional {
            match next {
                AndOr::And(p) | AndOr::Or(p) => self.pipeline(p),
            }
        }
    }

    fn pipeline(&mut self, pipeline: &Pipeline) {
        for command in &pipeline.seq {
            self.command(command);
        }
    }

    fn command(&mut self, command: &Command) {
        match command {
            Command::Simple(simple) => self.simple(simple),
            Command::Compound(compound, _) => self.compound(compound),
            Command::Function(function) => self.compound(&function.body.0),
            Command::ExtendedTest(..) => {}
        }
    }

    fn compound(&mut self, compound: &CompoundCommand) {
        match compound {
            CompoundCommand::BraceGroup(c) => self.list(&c.list),
            CompoundCommand::Subshell(c) => self.list(&c.list),
            CompoundCommand::ForClause(c) => {
                for word in c.values.iter().flatten() {
                    self.substitutions(word);
                }
                self.list(&c.body.list);
            }
            CompoundCommand::ArithmeticForClause(c) => self.list(&c.body.list),
            CompoundCommand::CaseClause(c) => {
                self.substitutions(&c.value);
                for item in &c.cases {
                    if let Some(list) = &item.cmd {
                        self.list(list);
                    }
                }
            }
            CompoundCommand::IfClause(c) => {
                self.list(&c.condition);
                self.list(&c.then);
                for branch in c.elses.iter().flatten() {
                    if let Some(condition) = &branch.condition {
                        self.list(condition);
                    }
                    self.list(&branch.body);
                }
            }
            CompoundCommand::WhileClause(c) | CompoundCommand::UntilClause(c) => {
                self.list(&c.0);
                self.list(&c.1.list);
            }
            CompoundCommand::Coprocess(c) => self.command(&c.body),
            CompoundCommand::Arithmetic(_) => {}
        }
    }

    fn simple(&mut self, command: &SimpleCommand) {
        let mut words: Vec<&Word> = Vec::new();
        let prefix = command.prefix.iter().flat_map(|p| p.0.iter());
        let suffix = command.suffix.iter().flat_map(|s| s.0.iter());
        let mut suffix_words: Vec<&Word> = Vec::new();
        for item in prefix {
            match item {
                CommandPrefixOrSuffixItem::Word(w) => words.push(w),
                other => self.item(other),
            }
        }
        if let Some(word) = &command.word_or_name {
            words.push(word);
        }
        for item in suffix {
            match item {
                CommandPrefixOrSuffixItem::Word(w) => suffix_words.push(w),
                other => self.item(other),
            }
        }
        for word in words.iter().chain(suffix_words.iter()) {
            self.substitutions(word);
        }
        let declares = words
            .last()
            .is_some_and(|w| DECLARERS.contains(&self.literal(w).as_str()));
        if declares {
            for word in &suffix_words {
                if let Some((name, value)) = word.value.split_once('=') {
                    let points = self.points_to_hippo(value);
                    self.vars.insert(name.to_string(), points);
                }
            }
            return;
        }
        let all: Vec<&Word> = words.into_iter().chain(suffix_words).collect();
        if let Some(exe) = self.executable(&all)
            && self.is_hippo_word(exe)
        {
            self.found = true;
        }
    }

    fn item(&mut self, item: &CommandPrefixOrSuffixItem) {
        match item {
            CommandPrefixOrSuffixItem::AssignmentWord(assignment, _) => {
                if let AssignmentValue::Scalar(value) = &assignment.value {
                    self.substitutions(value);
                    if let AssignmentName::VariableName(name) = &assignment.name {
                        let points = self.points_to_hippo(&value.value);
                        self.vars.insert(name.clone(), points);
                    }
                }
            }
            CommandPrefixOrSuffixItem::ProcessSubstitution(_, sub) => self.list(&sub.list),
            CommandPrefixOrSuffixItem::Word(w) => self.substitutions(w),
            CommandPrefixOrSuffixItem::IoRedirect(_) => {}
        }
    }

    /// 래퍼와 그 옵션, `env`의 `이름=값`을 건너뛴 실행 단어.
    fn executable<'a>(&self, words: &[&'a Word]) -> Option<&'a Word> {
        let mut rest = words.iter();
        let mut wrapped = false;
        for word in rest.by_ref() {
            let text = self.literal(word);
            if WRAPPERS.contains(&text.as_str()) {
                wrapped = true;
                continue;
            }
            if wrapped && (text.starts_with('-') || text.contains('=')) {
                continue;
            }
            return Some(word);
        }
        None
    }

    /// `$( )`와 백틱 안의 명령도 본다.
    fn substitutions(&mut self, word: &Word) {
        let Ok(pieces) = brush_parser::word::parse(&word.value, &self.options) else {
            return;
        };
        let mut inner = Vec::new();
        collect_substitutions(&pieces, &mut inner);
        for command in inner {
            if runs_hippo_with(&command, &self.vars) {
                self.found = true;
            }
        }
    }

    fn is_hippo_word(&self, word: &Word) -> bool {
        let Ok(pieces) = brush_parser::word::parse(&word.value, &self.options) else {
            return false;
        };
        if let Some(name) = sole_variable(&pieces) {
            return self.vars.get(&name).copied().unwrap_or(false);
        }
        last_component(&render(&pieces)).is_some_and(|name| name == HIPPO_EXE)
    }

    fn points_to_hippo(&self, value: &str) -> bool {
        brush_parser::word::parse(value, &self.options).is_ok_and(|pieces| {
            last_component(&render(&pieces)).is_some_and(|name| name == HIPPO_EXE)
        })
    }

    fn literal(&self, word: &Word) -> String {
        brush_parser::word::parse(&word.value, &self.options)
            .map(|pieces| render(&pieces))
            .unwrap_or_else(|_| word.value.clone())
    }
}

/// 같은 변수 정보를 이어받아 치환 안의 명령을 판정한다.
fn runs_hippo_with(command: &str, vars: &HashMap<String, bool>) -> bool {
    let options = ParserOptions::default();
    let mut parser = Parser::new(BufReader::new(command.as_bytes()), &options);
    let Ok(program) = parser.parse_program() else {
        return first_word_is_hippo(command);
    };
    let mut walker = Walker {
        options,
        vars: vars.clone(),
        found: false,
    };
    for list in &program.complete_commands {
        walker.list(list);
    }
    walker.found
}

fn collect_substitutions(pieces: &[WordPieceWithSource], out: &mut Vec<String>) {
    for p in pieces {
        match &p.piece {
            WordPiece::CommandSubstitution(c) | WordPiece::BackquotedCommandSubstitution(c) => {
                out.push(c.clone());
            }
            WordPiece::DoubleQuotedSequence(inner)
            | WordPiece::GettextDoubleQuotedSequence(inner) => collect_substitutions(inner, out),
            _ => {}
        }
    }
}

/// 따옴표를 벗긴 글자 그대로의 값. 알 수 없는 확장은 `UNKNOWN`으로 둔다.
fn render(pieces: &[WordPieceWithSource]) -> String {
    let mut out = String::new();
    for p in pieces {
        match &p.piece {
            WordPiece::Text(t) | WordPiece::SingleQuotedText(t) | WordPiece::AnsiCQuotedText(t) => {
                out.push_str(t);
            }
            WordPiece::EscapeSequence(e) => out.push_str(e.trim_start_matches('\\')),
            WordPiece::DoubleQuotedSequence(inner)
            | WordPiece::GettextDoubleQuotedSequence(inner) => out.push_str(&render(inner)),
            _ => out.push(UNKNOWN),
        }
    }
    out
}

/// 마지막 `/` 뒤의 이름. 알 수 없는 확장이 섞여 있으면 `None`.
fn last_component(text: &str) -> Option<&str> {
    let name = text.rsplit('/').next()?;
    (!name.is_empty() && !name.contains(UNKNOWN)).then_some(name)
}

/// `$H`, `${H}`, `"$H"`처럼 변수 하나뿐인 단어의 변수 이름.
fn sole_variable(pieces: &[WordPieceWithSource]) -> Option<String> {
    match pieces {
        [single] => match &single.piece {
            WordPiece::ParameterExpansion(ParameterExpr::Parameter {
                parameter: Parameter::Named(name),
                ..
            }) => Some(name.clone()),
            WordPiece::DoubleQuotedSequence(inner) => sole_variable(inner),
            _ => None,
        },
        _ => None,
    }
}
