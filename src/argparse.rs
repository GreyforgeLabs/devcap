//! A focused port of CPython 3.14 `argparse` parsing semantics.
//!
//! It implements exactly the features devcap's parser uses — long/short
//! options with prefix abbreviation, `--opt=value`, `store`, `store_true`,
//! `help`, choices, type converters, a mutually exclusive group, and a
//! subparsers positional — following the structure of
//! `ArgumentParser._parse_known_args` so that edge cases (ambiguous prefixes,
//! `--`, negative-number-looking values, explicit arguments on flags, error
//! precedence) behave the same. Help and usage text are supplied verbatim.

use std::collections::{BTreeMap, HashMap};

use crate::pycompat::{is_decimal, repr};

/// Converted argument value.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Str(String),
    Float(f64),
    Int(i64),
    Bool(bool),
}

/// Type converter: `Err` carries an `ArgumentTypeError` message.
pub type Converter = fn(&str) -> Result<Val, String>;

/// Action kinds devcap uses.
#[derive(Clone)]
pub enum Kind {
    Help,
    StoreTrue,
    Store {
        choices: Option<&'static [&'static str]>,
        convert: Option<Converter>,
    },
}

/// An optional argument definition.
#[derive(Clone)]
pub struct OptAction {
    pub strings: &'static [&'static str],
    pub dest: &'static str,
    pub kind: Kind,
    pub default: Option<Val>,
}

/// A parser definition.
pub struct Parser {
    pub prog: &'static str,
    pub usage: &'static str,
    pub help: &'static str,
    pub actions: Vec<OptAction>,
    /// Each group lists indices into `actions`.
    pub mutex_groups: Vec<Vec<usize>>,
    /// Subcommands `(name, parser)` in registration order; dest `command`.
    pub subparsers: Vec<(&'static str, Parser)>,
}

/// Parsed values keyed by `dest`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Namespace {
    pub values: BTreeMap<&'static str, Val>,
    pub command: Option<String>,
    unrecognized: Vec<String>,
}

impl Namespace {
    pub fn str(&self, dest: &str) -> Option<&str> {
        match self.values.get(dest) {
            Some(Val::Str(s)) => Some(s),
            _ => None,
        }
    }

    pub fn float(&self, dest: &str) -> Option<f64> {
        match self.values.get(dest) {
            Some(Val::Float(f)) => Some(*f),
            _ => None,
        }
    }

    pub fn int(&self, dest: &str) -> Option<i64> {
        match self.values.get(dest) {
            Some(Val::Int(i)) => Some(*i),
            _ => None,
        }
    }

    pub fn flag(&self, dest: &str) -> bool {
        matches!(self.values.get(dest), Some(Val::Bool(true)))
    }
}

/// Early termination: help (`code` 0, stdout) or usage error (`code` 2, stderr).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Identity of an action within one parser: optional index or the
/// subparsers positional.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum ActionId {
    Opt(usize),
    Command,
}

/// `(action, option_string, sep, explicit_arg)` from `_parse_optional`.
#[derive(Clone, Debug)]
struct OptionTuple {
    action: Option<usize>,
    option_string: String,
    sep: Option<String>,
    explicit_arg: Option<String>,
}

fn partition_eq(s: &str) -> (String, Option<String>, Option<String>) {
    match s.find('=') {
        Some(i) => (
            s[..i].to_string(),
            Some("=".to_string()),
            Some(s[i + 1..].to_string()),
        ),
        None => (s.to_string(), None, None),
    }
}

impl Parser {
    fn option_strings(&self) -> impl Iterator<Item = (usize, &'static str)> + '_ {
        self.actions
            .iter()
            .enumerate()
            .flat_map(|(i, a)| a.strings.iter().map(move |s| (i, *s)))
    }

    fn lookup(&self, option: &str) -> Option<usize> {
        self.option_strings()
            .find(|(_, s)| *s == option)
            .map(|(i, _)| i)
    }

    fn action_name(&self, id: ActionId) -> String {
        match id {
            ActionId::Opt(i) => self.actions[i].strings.join("/"),
            ActionId::Command => "command".to_string(),
        }
    }

    /// `parser.error(message)`.
    pub fn error(&self, message: &str) -> Exit {
        Exit {
            code: 2,
            stdout: String::new(),
            stderr: format!("{}{}: error: {}\n", self.usage, self.prog, message),
        }
    }

    fn arg_error(&self, id: Option<ActionId>, message: &str) -> Exit {
        match id {
            Some(id) => self.error(&format!("argument {}: {}", self.action_name(id), message)),
            None => self.error(message),
        }
    }

    fn get_option_tuples(&self, arg: &str) -> Vec<OptionTuple> {
        let mut result = Vec::new();
        let chars: Vec<char> = arg.chars().collect();
        if chars.len() >= 2 && chars[0] == '-' && chars[1] == '-' {
            let (prefix, sep, explicit) = partition_eq(arg);
            for (i, opt) in self.option_strings() {
                if opt.starts_with(&prefix) {
                    result.push(OptionTuple {
                        action: Some(i),
                        option_string: opt.to_string(),
                        sep: sep.clone(),
                        explicit_arg: explicit.clone(),
                    });
                }
            }
        } else if chars.len() >= 2 && chars[0] == '-' {
            let (prefix, sep, explicit) = partition_eq(arg);
            let short_prefix: String = chars[..2].iter().collect();
            let short_explicit: String = chars[2..].iter().collect();
            for (i, opt) in self.option_strings() {
                if opt == short_prefix {
                    result.push(OptionTuple {
                        action: Some(i),
                        option_string: opt.to_string(),
                        sep: Some(String::new()),
                        explicit_arg: Some(short_explicit.clone()),
                    });
                } else if opt.starts_with(&prefix) {
                    result.push(OptionTuple {
                        action: Some(i),
                        option_string: opt.to_string(),
                        sep: sep.clone(),
                        explicit_arg: explicit.clone(),
                    });
                }
            }
        }
        result
    }

    fn parse_optional(&self, arg: &str) -> Option<Vec<OptionTuple>> {
        if !arg.starts_with('-') {
            return None;
        }
        if let Some(i) = self.lookup(arg) {
            return Some(vec![OptionTuple {
                action: Some(i),
                option_string: arg.to_string(),
                sep: None,
                explicit_arg: None,
            }]);
        }
        if arg.chars().count() == 1 {
            return None;
        }
        let (option_string, sep, explicit) = partition_eq(arg);
        if sep.is_some()
            && let Some(i) = self.lookup(&option_string)
        {
            return Some(vec![OptionTuple {
                action: Some(i),
                option_string,
                sep,
                explicit_arg: explicit,
            }]);
        }
        let tuples = self.get_option_tuples(arg);
        if !tuples.is_empty() {
            return Some(tuples);
        }
        // `_negative_number_matcher = re.compile(r'-\.?\d')`; devcap has no
        // negative-number-like options, so such strings are positional.
        let rest: Vec<char> = arg.chars().skip(1).collect();
        let looks_negative = match rest.first() {
            Some('.') => rest.get(1).is_some_and(|&c| is_decimal(c)),
            Some(&c) => is_decimal(c),
            None => false,
        };
        if looks_negative || arg.contains(' ') {
            return None;
        }
        Some(vec![OptionTuple {
            action: None,
            option_string: arg.to_string(),
            sep: None,
            explicit_arg: None,
        }])
    }

    fn arg_count(&self, action: usize) -> usize {
        match self.actions[action].kind {
            Kind::Store { .. } => 1,
            Kind::Help | Kind::StoreTrue => 0,
        }
    }

    /// `parse_args`: returns the namespace or an early exit.
    pub fn parse_args(&self, args: &[String]) -> Result<Namespace, Exit> {
        let mut ns = Namespace::default();
        let mut extras = self.parse_known_args(args, &mut ns)?;
        extras.append(&mut ns.unrecognized);
        if !extras.is_empty() {
            return Err(self.error(&format!("unrecognized arguments: {}", extras.join(" "))));
        }
        Ok(ns)
    }

    fn parse_known_args(&self, args: &[String], ns: &mut Namespace) -> Result<Vec<String>, Exit> {
        for action in &self.actions {
            if let Some(default) = &action.default
                && !ns.values.contains_key(action.dest)
            {
                ns.values.insert(action.dest, default.clone());
            }
        }

        let mut conflicts: HashMap<usize, Vec<usize>> = HashMap::new();
        for group in &self.mutex_groups {
            for (i, &a) in group.iter().enumerate() {
                let entry = conflicts.entry(a).or_default();
                entry.extend(&group[..i]);
                entry.extend(&group[i + 1..]);
            }
        }

        let mut option_indices: HashMap<usize, Vec<OptionTuple>> = HashMap::new();
        let mut pattern: Vec<char> = Vec::with_capacity(args.len());
        let mut iter = args.iter().enumerate();
        while let Some((i, arg)) = iter.next() {
            if arg == "--" {
                pattern.push('-');
                pattern.extend(iter.by_ref().map(|_| 'A'));
                break;
            }
            match self.parse_optional(arg) {
                None => pattern.push('A'),
                Some(tuples) => {
                    option_indices.insert(i, tuples);
                    pattern.push('O');
                }
            }
        }

        let mut seen_non_default: Vec<ActionId> = Vec::new();
        let mut extras: Vec<String> = Vec::new();
        let mut positionals_left = !self.subparsers.is_empty();

        let max_option_index = option_indices.keys().max().copied();
        let mut start = 0usize;
        while max_option_index.is_some_and(|m| start <= m) {
            let max = max_option_index.unwrap_or(0);
            let mut next = start;
            while next <= max && !option_indices.contains_key(&next) {
                next += 1;
            }
            if start != next {
                let end = self.consume_positionals(
                    args,
                    &pattern,
                    start,
                    &mut positionals_left,
                    ns,
                    &mut seen_non_default,
                )?;
                if end > start {
                    start = end;
                    continue;
                }
                start = end;
            }
            if !option_indices.contains_key(&start) {
                extras.extend_from_slice(&args[start..next]);
                start = next;
            }
            start = self.consume_optional(
                args,
                &pattern,
                start,
                &option_indices,
                &conflicts,
                ns,
                &mut seen_non_default,
                &mut extras,
            )?;
        }
        let stop = self.consume_positionals(
            args,
            &pattern,
            start,
            &mut positionals_left,
            ns,
            &mut seen_non_default,
        )?;
        extras.extend_from_slice(&args[stop.min(args.len())..]);
        Ok(extras)
    }

    fn consume_positionals(
        &self,
        args: &[String],
        pattern: &[char],
        start: usize,
        positionals_left: &mut bool,
        ns: &mut Namespace,
        seen_non_default: &mut Vec<ActionId>,
    ) -> Result<usize, Exit> {
        if !*positionals_left {
            return Ok(start);
        }
        // nargs=PARSER pattern '(-*A[-AO]*)'.
        let rest = &pattern[start.min(pattern.len())..];
        let dashes = rest.iter().take_while(|&&c| c == '-').count();
        if rest.get(dashes) != Some(&'A') {
            return Ok(start);
        }
        let count = rest.len();
        let mut values: Vec<String> = args[start..start + count].to_vec();
        if rest.first() == Some(&'-') {
            values.remove(0);
        }
        *positionals_left = false;
        seen_non_default.push(ActionId::Command);
        let name = values[0].clone();
        let Some((_, sub)) = self.subparsers.iter().find(|(n, _)| *n == name) else {
            let choices = self
                .subparsers
                .iter()
                .map(|(n, _)| repr(n))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(self.arg_error(
                Some(ActionId::Command),
                &format!("invalid choice: {} (choose from {choices})", repr(&name)),
            ));
        };
        ns.command = Some(name);
        let mut sub_ns = Namespace::default();
        let sub_extras = sub.parse_known_args(&values[1..], &mut sub_ns)?;
        for (k, v) in sub_ns.values {
            ns.values.insert(k, v);
        }
        ns.unrecognized.extend(sub_extras);
        Ok(start + count)
    }

    #[allow(clippy::too_many_arguments)]
    fn consume_optional(
        &self,
        args: &[String],
        pattern: &[char],
        start: usize,
        option_indices: &HashMap<usize, Vec<OptionTuple>>,
        conflicts: &HashMap<usize, Vec<usize>>,
        ns: &mut Namespace,
        seen_non_default: &mut Vec<ActionId>,
        extras: &mut Vec<String>,
    ) -> Result<usize, Exit> {
        let tuples = &option_indices[&start];
        if tuples.len() > 1 {
            let options = tuples
                .iter()
                .map(|t| t.option_string.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(self.arg_error(
                None,
                &format!("ambiguous option: {} could match {options}", args[start]),
            ));
        }
        let first = tuples[0].clone();
        let mut action = first.action;
        let mut option_string = first.option_string;
        let mut sep = first.sep;
        let mut explicit_arg = first.explicit_arg;
        let mut action_tuples: Vec<(usize, Vec<String>)> = Vec::new();
        let stop;
        loop {
            let Some(act) = action else {
                extras.push(args[start].clone());
                return Ok(start + 1);
            };
            if let Some(explicit) = explicit_arg.clone() {
                let arg_count = self.arg_count(act);
                let second = option_string.chars().nth(1);
                if arg_count == 0 && second != Some('-') && !explicit.is_empty() {
                    let sep_truthy = sep.as_deref().is_some_and(|s| !s.is_empty());
                    if sep_truthy || explicit.starts_with('-') {
                        return Err(self.arg_error(
                            Some(ActionId::Opt(act)),
                            &format!("ignored explicit argument {}", repr(&explicit)),
                        ));
                    }
                    action_tuples.push((act, Vec::new()));
                    let first_char = explicit.chars().next().unwrap_or_default();
                    option_string = format!("-{first_char}");
                    match self.lookup(&option_string) {
                        Some(next_action) => {
                            action = Some(next_action);
                            let remainder: String = explicit.chars().skip(1).collect();
                            if remainder.is_empty() {
                                sep = None;
                                explicit_arg = None;
                            } else if let Some(stripped) = remainder.strip_prefix('=') {
                                sep = Some("=".to_string());
                                explicit_arg = Some(stripped.to_string());
                            } else {
                                sep = Some(String::new());
                                explicit_arg = Some(remainder);
                            }
                        }
                        None => {
                            extras.push(format!("-{explicit}"));
                            stop = start + 1;
                            break;
                        }
                    }
                } else if arg_count == 1 {
                    stop = start + 1;
                    action_tuples.push((act, vec![explicit]));
                    break;
                } else {
                    return Err(self.arg_error(
                        Some(ActionId::Opt(act)),
                        &format!("ignored explicit argument {}", repr(&explicit)),
                    ));
                }
            } else {
                let begin = start + 1;
                let arg_count = self.arg_count(act);
                if arg_count == 1 && pattern.get(begin) != Some(&'A') {
                    return Err(self.arg_error(Some(ActionId::Opt(act)), "expected one argument"));
                }
                stop = begin + arg_count;
                action_tuples.push((act, args[begin..stop].to_vec()));
                break;
            }
        }
        for (act, values) in action_tuples {
            self.take_action(act, &values, conflicts, ns, seen_non_default)?;
        }
        Ok(stop)
    }

    fn take_action(
        &self,
        act: usize,
        values: &[String],
        conflicts: &HashMap<usize, Vec<usize>>,
        ns: &mut Namespace,
        seen_non_default: &mut Vec<ActionId>,
    ) -> Result<(), Exit> {
        let action = &self.actions[act];
        let id = ActionId::Opt(act);
        let converted = match &action.kind {
            Kind::Store { choices, convert } => {
                let raw = &values[0];
                let value = match convert {
                    Some(f) => f(raw).map_err(|msg| self.arg_error(Some(id), &msg))?,
                    None => Val::Str(raw.clone()),
                };
                if let (Some(choices), Val::Str(s)) = (choices, &value)
                    && !choices.contains(&s.as_str())
                {
                    let list = choices
                        .iter()
                        .map(|c| repr(c))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(self.arg_error(
                        Some(id),
                        &format!("invalid choice: {} (choose from {list})", repr(s)),
                    ));
                }
                Some(value)
            }
            Kind::StoreTrue => Some(Val::Bool(true)),
            Kind::Help => None,
        };
        seen_non_default.push(id);
        if let Some(conflicting) = conflicts.get(&act) {
            for &other in conflicting {
                if seen_non_default.contains(&ActionId::Opt(other)) {
                    return Err(self.arg_error(
                        Some(id),
                        &format!(
                            "not allowed with argument {}",
                            self.action_name(ActionId::Opt(other))
                        ),
                    ));
                }
            }
        }
        match converted {
            Some(value) => {
                ns.values.insert(action.dest, value);
                Ok(())
            }
            None => Err(Exit {
                code: 0,
                stdout: self.help.to_string(),
                stderr: String::new(),
            }),
        }
    }
}
