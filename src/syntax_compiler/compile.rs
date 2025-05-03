use crate::syntax_compiler::parse;
use snafu::{ResultExt, Snafu, ensure};
use std::collections::HashMap;
use std::num::{NonZeroU8, NonZeroU16, ParseIntError};
// todo: deduplicate regexes
// todo: deduplicates rules, too, actually
// todo: intern strings
// todo: merge strings into one big string
//       and use offsets into that string, in roughly this style:
//       https://matklad.github.io/2020/03/22/fast-simple-rust-interner.html
// todo: broad alignment with syntect pub vocabulary (eg Bundle -> SyntaxSet)
// todo: investigate if caching lines (accounting for the state) is worth it
// todo: limit time/line length

// todo: in stage 1:
//       - references stay references (parsed kind of reference + name)
//       - registries should go into a separate vector and point to rule IDs (should be OK)
//       - rules should carry a stack (vector) of repositories applicable to them, since
//         after compilation nesting disappears

// todo: patch the rules

// todo: injections

// todo: linker will
//       1) remove Nones from SyntaxDefinitions
//       2) resolve references
//       3) deduplicate rules and remove noop rules
//       4) deduplicate regexes
//       5) inline everything

#[derive(Debug, Snafu)]
pub(crate) enum Error {
    RepositoryStackOverflow,
    #[snafu(display("failed to deserialize capture index \"{}\"", index))]
    UnparseableCaptureIndex {
        index: String,
        source: ParseIntError,
    },
}

macro_rules! impl_idx_conversion {
    ($type:ident, $int_type:ident, $int_nonzero_type:ident) => {
        impl $type {
            fn to_idx(self) -> usize {
                self.0.get() as usize - 1
            }

            fn from_idx(idx: usize) -> Self {
                // todo: some of those are actually valid grammar errors vs bugs,
                //       I should probably return proper errors
                Self(
                    $int_nonzero_type::new(
                        $int_type::try_from(idx.checked_add(1).unwrap()).unwrap(),
                    )
                    .unwrap(),
                )
            }
        }
    };
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
struct RuleId(NonZeroU16);

#[derive(Debug, Copy, Clone)]
struct RegexId(NonZeroU16);

#[derive(Debug, Copy, Clone)]
struct PartialRegexId(NonZeroU16);

#[derive(Debug, Copy, Clone)]
struct RepositoryId(NonZeroU8);

impl_idx_conversion!(RuleId, u16, NonZeroU16);
impl_idx_conversion!(RegexId, u16, NonZeroU16);
impl_idx_conversion!(PartialRegexId, u16, NonZeroU16);
impl_idx_conversion!(RepositoryId, u8, NonZeroU8);

// separate class just to make code clearer later when I parse/intern it
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct ScopeName(String);

impl From<parse::ScopeName> for ScopeName {
    fn from(value: parse::ScopeName) -> Self {
        Self(value.0)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Repository {
    pub(crate) rules: HashMap<ScopeName, RuleId>,
}

// more convenient than copy and is still very small
const MAX_REPOSITORY_STACK_DEPTH: u8 = 4;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RepositoryStack {
    pub(crate) stack: [Option<RepositoryId>; MAX_REPOSITORY_STACK_DEPTH as usize],
    pub(crate) capacity: u8,
}

impl RepositoryStack {
    fn empty() -> Self {
        Default::default()
    }

    fn push(mut self, repository_id: RepositoryId) -> Result<Self, Error> {
        ensure!(
            self.capacity < MAX_REPOSITORY_STACK_DEPTH,
            RepositoryStackOverflowSnafu
        );

        self.stack[self.capacity as usize] = Some(repository_id);
        self.capacity += 1;
        Ok(self)
    }

    fn pop(mut self) -> Result<(Self, RepositoryId), Error> {
        // unlike ensure! above, this shouldn't depend on a grammar and thus if it
        // ever happens, it's a Horrible Bug
        assert!(self.capacity > 0);

        let popped = self.stack[self.capacity as usize - 1].take().unwrap();
        self.capacity -= 1;
        Ok((self, popped))
    }
}

// Grammars need to be "compiled" as a bundle, since they might refer to each other
// via "include" fields rules
// TODO: it might make sense to have a separate type for injections
#[derive(Debug, Clone)]
pub(crate) struct SyntaxSet(pub(crate) Vec<SyntaxDefinition>);

#[derive(Debug, Clone)]
pub(crate) struct SyntaxDefinition {
    scope_name: ScopeName,
    rules: Vec<Option<Rule>>,
    regexes: Vec<parse::RegExpString>,
    // those regexes might need substitutions
    partial_regexes: Vec<parse::PartialRegExpString>,
    repositories: Vec<Option<Repository>>,
}

impl SyntaxDefinition {
    pub(crate) fn compile(raw: parse::SyntaxDefinition) -> Result<Self, Error> {
        let mut syntax = Self {
            scope_name: raw.scope_name.into(),
            rules: Vec::new(),
            regexes: Vec::new(),
            partial_regexes: Vec::new(),
            repositories: Vec::new(),
        };

        let root_rule_id = syntax.compile_rule(
            RepositoryStack::empty(),
            parse::Rule {
                patterns: Some(raw.patterns),
                repository: raw.repository,
                ..Default::default()
            },
        )?;

        assert_eq!(root_rule_id, RuleId::from_idx(0));

        Ok(syntax)
    }

    fn compile_repository(
        &mut self,
        repository_stack: RepositoryStack,
        raw_repository: parse::Repository,
    ) -> Result<RepositoryId, Error> {
        let new_id = RepositoryId::from_idx(self.repositories.len());

        // push a temporary None to reserve the position, recursive
        // calls might add more before the repository is ready
        self.repositories.push(None);

        let new_repository_stack = repository_stack.push(new_id)?;

        let repository = raw_repository
            .0
            .into_iter()
            .map(|(name, raw_rule)| {
                Ok((
                    ScopeName(name.to_string()),
                    self.compile_rule(new_repository_stack, raw_rule.clone())?,
                ))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;

        self.repositories[new_id.to_idx()] = Some(Repository { rules: repository });

        Ok(new_id)
    }

    fn compile_rule(
        &mut self,
        repository_stack: RepositoryStack,
        raw_rule: parse::Rule,
    ) -> Result<RuleId, Error> {
        // closely follows the logic in
        // https://github.com/microsoft/vscode-textmate/blob/f03a6a8790af81372d0e81facae75554ec5e97ef/src/rule.ts#L389-L447
        // to match implicit priority

        let new_id = RuleId::from_idx(self.rules.len());

        // push a temporary None to reserve the position, recursive
        // calls might add more before we have the rule ready
        self.rules.push(None);

        let rule = {
            if let Some(match_) = raw_rule.match_ {
                Rule::MatchRule(MatchRule {
                    id: new_id,
                    name: raw_rule.name.map(ScopeName::from),
                    repository_stack,
                    match_: self.compile_regex(match_),
                    captures: raw_rule
                        .captures
                        .map(|c| self.compile_captures(repository_stack, c))
                        .transpose()?
                        .flatten(),
                })
            } else if let Some(begin) = raw_rule.begin {
                if let Some(while_) = raw_rule.while_ {
                    Rule::BeginWhileRule(BeginWhileRule {
                        id: new_id,
                        name: raw_rule.name.map(ScopeName::from),
                        repository_stack,
                        content_name: raw_rule.content_name.map(ScopeName::from),
                        begin: self.compile_regex(begin),
                        begin_captures: raw_rule
                            .begin_captures
                            .map(|c| self.compile_captures(repository_stack, c))
                            .transpose()?
                            .flatten(),
                        while_: self.compile_partial_regex(while_),
                        while_captures: raw_rule
                            .while_captures
                            .map(|c| self.compile_captures(repository_stack, c))
                            .transpose()?
                            .flatten(),
                        patterns: raw_rule
                            .patterns
                            .map(|p| self.compile_patterns(repository_stack, p))
                            .transpose()?,
                    })
                } else {
                    Rule::BeginEndRule(BeginEndRule {
                        id: new_id,
                        name: raw_rule.name.map(ScopeName::from),
                        repository_stack,
                        content_name: raw_rule.content_name.map(ScopeName::from),
                        begin: self.compile_regex(begin),
                        begin_captures: raw_rule
                            .begin_captures
                            .map(|c| self.compile_captures(repository_stack, c))
                            .transpose()?
                            .flatten(),
                        end: raw_rule.end.map(|e| self.compile_partial_regex(e)),
                        end_captures: raw_rule
                            .end_captures
                            .map(|c| self.compile_captures(repository_stack, c))
                            .transpose()?
                            .flatten(),
                        apply_end_pattern_last: raw_rule.apply_end_pattern_last.unwrap_or(false),
                        patterns: raw_rule
                            .patterns
                            .map(|p| self.compile_patterns(repository_stack, p))
                            .transpose()?,
                    })
                }
            } else {
                // add in a repository into the stack if raw_rule.repository exists
                let repository_stack = if let Some(raw_repository) = raw_rule.repository {
                    let new_repository_id =
                        self.compile_repository(repository_stack, raw_repository)?;

                    repository_stack.push(new_repository_id)?
                } else {
                    repository_stack
                };

                // vscode-textmate does something funny here:
                // - if patterns are NOT present and includes are, it moves includes to patterns;
                // - however, if patterns ARE present, includes are ignored
                // https://github.com/microsoft/vscode-textmate/blob/f03a6a8790af81372d0e81facae75554ec5e97ef/src/rule.ts#L404
                let patterns = raw_rule.patterns.or_else(|| {
                    raw_rule.include.map(|include| {
                        vec![parse::Rule {
                            include: Some(include),
                            ..parse::Rule::default()
                        }]
                    })
                });

                // If patterns are None or empty, the rule is useless. However, by this point
                // it's already allocated an ID and other rules might have been recursively
                // allocated, too, so removing it is a bit painful, and it's easier to have
                // a special noop rule
                match patterns {
                    Some(patterns) if !patterns.is_empty() => {
                        Rule::IncludeOnlyRule(IncludeOnlyRule {
                            id: new_id,
                            name: raw_rule.name.map(ScopeName::from),
                            repository_stack,
                            content_name: raw_rule.content_name.map(ScopeName::from),
                            patterns: self.compile_patterns(repository_stack, patterns)?,
                        })
                    }
                    _ => Rule::NoopRule,
                }
            }
        };

        self.rules[new_id.to_idx()] = Some(rule);
        Ok(new_id)
    }

    fn compile_regex(&mut self, regex: parse::RegExpString) -> RegexId {
        let new_id = RegexId::from_idx(self.regexes.len());
        self.regexes.push(regex);
        new_id
    }

    fn compile_partial_regex(&mut self, regex: parse::PartialRegExpString) -> PartialRegexId {
        let new_id = PartialRegexId::from_idx(self.partial_regexes.len());
        self.partial_regexes.push(regex);
        new_id
    }

    fn compile_captures(
        &mut self,
        repository_stack: RepositoryStack,
        raw_captures: parse::Captures,
    ) -> Result<Option<Captures>, Error> {
        let max_capture = raw_captures
            .0
            .keys()
            .map(|k| k.parse::<usize>().unwrap_or(0))
            .max();
        let max_capture = if let Some(max_capture) = max_capture {
            max_capture
        } else {
            return Ok(None);
        };

        let mut captures: Vec<Option<RuleId>> = vec![None; max_capture + 1];

        for (key, raw_rule) in raw_captures.0 {
            let idx = key
                .parse::<usize>()
                .with_context(|_| UnparseableCaptureIndexSnafu { index: key.clone() })?;
            captures[idx] = Some(self.compile_rule(repository_stack, raw_rule)?);
        }

        Ok(Some(Captures(captures)))
    }

    fn compile_patterns(
        &mut self,
        repository_stack: RepositoryStack,
        raw_patterns: Vec<parse::Rule>,
    ) -> Result<Vec<RuleIdOrReference>, Error> {
        raw_patterns
            .into_iter()
            .map(|raw_rule| {
                if let Some(include) = raw_rule.include {
                    // vscode ignores other rule contents is there's an include
                    // https://github.com/microsoft/vscode-textmate/blob/f03a6a8790af81372d0e81facae75554ec5e97ef/src/rule.ts#L495
                    Ok(RuleIdOrReference::Reference((&include).into()))
                } else {
                    let rule_id = self.compile_rule(repository_stack, raw_rule)?;
                    Ok(RuleIdOrReference::RuleId(rule_id))
                }
            })
            .collect::<Result<Vec<_>, _>>()
    }
}

#[derive(Debug, Clone)]
struct MatchRule {
    id: RuleId,
    // todo: intern
    name: Option<ScopeName>,
    // todo: trace where the rule came from; probably can be a recursive pointer to RuleId
    // path: ???,
    repository_stack: RepositoryStack,
    match_: RegexId,
    // not all captures might be present => capture N is at index N
    // TODO: measure real life capacities; should be a tinyvec or something like that
    captures: Option<Captures>,
}

#[derive(Debug, Clone)]
struct IncludeOnlyRule {
    id: RuleId,
    name: Option<ScopeName>,
    repository_stack: RepositoryStack,
    content_name: Option<ScopeName>,
    patterns: Vec<RuleIdOrReference>,
}

#[derive(Debug, Clone)]
struct BeginWhileRule {
    id: RuleId,
    name: Option<ScopeName>,
    repository_stack: RepositoryStack,
    content_name: Option<ScopeName>,
    begin: RegexId,
    begin_captures: Option<Captures>,
    while_: PartialRegexId,
    while_captures: Option<Captures>,
    patterns: Option<Vec<RuleIdOrReference>>,
}

#[derive(Debug, Clone)]
struct BeginEndRule {
    id: RuleId,
    name: Option<ScopeName>,
    repository_stack: RepositoryStack,
    content_name: Option<ScopeName>,
    begin: RegexId,
    begin_captures: Option<Captures>,
    // begin/end patterns might not have the final pattern
    end: Option<PartialRegexId>,
    end_captures: Option<Captures>,
    apply_end_pattern_last: bool,
    patterns: Option<Vec<RuleIdOrReference>>,
}

#[derive(Debug, Clone)]
enum Rule {
    MatchRule(MatchRule),
    IncludeOnlyRule(IncludeOnlyRule),
    BeginWhileRule(BeginWhileRule),
    BeginEndRule(BeginEndRule),
    NoopRule,
}

#[derive(Debug, Clone)]
struct Captures(Vec<Option<RuleId>>);

// per vscode-textmate:
//  Allowed values:
//  * Scope Name, e.g. `source.ts`
//  * Top level scope reference, e.g. `source.ts#entity.name.class`
//  * Relative scope reference, e.g. `#entity.name.class`
//  * self, e.g. `$self`
//  * base, e.g. `$base`
// https://github.com/microsoft/vscode-textmate/blob/f03a6a8790af81372d0e81facae75554ec5e97ef/src/rawGrammar.ts#L21
// also:
// `"$self"` includes the entire grammar file again.
// `"$base"` includes the top level grammar file. (When embedded inside other grammars).
// `"#..."` includes a repository rule in the same grammar file.
// `"source..."` includes another grammar file with the [scopeName](#scopename).
// `"source...#..."` includes a repository rule in the other grammar file.
// https://github.com/RedCMD/TmLanguage-Syntax-Highlighter/blob/a365719a50bf2b008da8d319acab143227e56dee/documentation/rules.md?plain=1#L109
#[derive(Debug, Clone)]
enum Reference {
    Base,
    Self_,
    /// Include entire another grammar file with (scopeName = scope)
    TopLevel {
        scope: ScopeName,
    },
    /// Repository rule in the same grammar file (note that repository stacking applies)
    Relative {
        rule: ScopeName,
    },
    /// Repository rule in another grammar file (scopeName = scope)
    TopLevelRepository {
        scope: ScopeName,
        rule: ScopeName,
    },
}

// todo: note that textmate ignores failing links and excludes patterns if links are missing
impl From<&parse::IncludeString> for Reference {
    fn from(s: &parse::IncludeString) -> Self {
        match s.0.as_str() {
            "$base" => Reference::Base,
            "$self" => Reference::Self_,
            other if other.starts_with("#") => Reference::Relative {
                rule: ScopeName(other[1..].to_string()),
            },
            other if other.contains("#") => {
                let (scope, rule) = other.split_once('#').unwrap();
                Reference::TopLevelRepository {
                    scope: ScopeName(scope.to_string()),
                    rule: ScopeName(rule.to_string()),
                }
            }
            other => Reference::TopLevel {
                scope: ScopeName(other.to_string()),
            },
        }
    }
}

#[derive(Debug, Clone)]
enum RuleIdOrReference {
    RuleId(RuleId),
    Reference(Reference),
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::{Report, Whatever};
    use std::path::Path;
    use test_case::test_case;

    #[test]
    fn can_compile_simple_syntax() {
        let include_digits = parse::Rule {
            include: Some(parse::IncludeString("#digits".to_string())),
            ..Default::default()
        };
        let include_ws = parse::Rule {
            include: Some(parse::IncludeString("#whitespace".to_string())),
            ..Default::default()
        };

        let rule_digits = parse::Rule {
            match_: Some(parse::RegExpString("\\d+".to_string())),
            name: Some(parse::ScopeName("digits".to_string())),
            ..Default::default()
        };
        let rule_ws = parse::Rule {
            begin: Some(parse::RegExpString("\\s".to_string())),
            end: Some(parse::PartialRegExpString("\\S".to_string())),
            name: Some(parse::ScopeName("whitespace".to_string())),
            ..Default::default()
        };

        let repository = parse::Repository(HashMap::from([
            ("digits".to_string(), rule_digits),
            ("whitespace".to_string(), rule_ws),
        ]));

        let parsed_syntax = parse::SyntaxDefinition {
            scope_name: parse::ScopeName("source.simple".to_string()),
            patterns: vec![include_digits, include_ws],
            repository: Some(repository),
            injections: None,
            injection_selector: None,
            inject_to: None,
        };

        let compiled_syntax = SyntaxDefinition::compile(parsed_syntax).unwrap();
    }

    const RAW_GRAMMARS_PATH: &str = "tests/textmate-grammars-themes/packages/tm-grammars/raw";
    const CLEANED_GRAMMARS_PATH: &str =
        "tests/textmate-grammars-themes/packages/tm-grammars/grammars";
    const PROBLEMATIC_GRAMMARS: &[&str] = &[
        "wikitext.json",
        "stata.json",
        "racket.json",
        "xml.json",
        "jinja.json",
        "smalltalk.json",
        // fails to compile
        "latex.json",
    ];

    #[test_case(RAW_GRAMMARS_PATH ; "raw")]
    #[test_case(CLEANED_GRAMMARS_PATH ; "cleaned")]
    fn can_load_grammars(grammars_path: &'static str) -> Report<Whatever> {
        Report::capture(|| {
            let cargo_dir = env!("CARGO_MANIFEST_DIR");
            let raw_grammars_path = Path::new(cargo_dir).join(grammars_path);

            for entry in raw_grammars_path.read_dir().unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if PROBLEMATIC_GRAMMARS.iter().any(|&g| path.ends_with(g)) {
                    continue;
                }
                let json = std::fs::read_to_string(&path).unwrap();

                let parsed = parse::SyntaxDefinition::from_json(&json)
                    .with_whatever_context(|_| format!("parsing {} failed", path.display()))?;

                let _sd = SyntaxDefinition::compile(parsed)
                    .with_whatever_context(|_| format!("compiling {} failed", path.display()))?;
            }

            Ok(())
        })
    }
}
