use serde_derive::Deserialize;
use snafu::prelude::*;
use std::collections::HashMap;

#[derive(Debug, Snafu)]
pub(crate) enum Error {
    #[snafu(display("failed to deserialize json at \"{}\"", path))]
    Json {
        path: String,
        #[snafu(source(from(serde_path_to_error::Error<serde_json::Error>, serde_path_to_error::Error::into_inner
        )))]
        source: serde_json::Error,
    },
}

// modelled after https://github.com/microsoft/vscode-textmate/blob/f03a6a8790af81372d0e81facae75554ec5e97ef/src/rawGrammar.ts
// and https://github.com/RedCMD/TmLanguage-Syntax-Highlighter/blob/main/documentation/rules.md

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SyntaxDefinition {
    // expected to be globally unique
    pub(crate) scope_name: ScopeName,
    pub(crate) patterns: Vec<Rule>,

    pub(crate) repository: Option<Repository>,

    pub(crate) injections: Option<HashMap<String, Rule>>,
    pub(crate) injection_selector: Option<String>,

    // not in https://github.com/RedCMD/TmLanguage-Syntax-Highlighter/blob/main/documentation/rules.md
    // but is present in some real world grammars; maybe we should ignore it?
    pub(crate) inject_to: Option<Vec<String>>,
    //
    // fileTypes, name, and firstLineMatch are present in vscode,
    // but are apparently ignored, so no point parsing them
}

impl SyntaxDefinition {
    pub(crate) fn from_json(json: &str) -> Result<Self, Error> {
        let des = &mut serde_json::Deserializer::from_str(json);

        serde_path_to_error::deserialize(des).with_context(|e| JsonSnafu {
            path: e.path().to_string(),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub(crate) struct Repository(pub(crate) HashMap<String, Rule>);

// per vscode-textmate:
//  Allowed values:
//  * Scope Name, e.g. `source.ts`
//  * Top level scope reference, e.g. `source.ts#entity.name.class`
//  * Relative scope reference, e.g. `#entity.name.class`
//  * self, e.g. `$self`
//  * base, e.g. `$base`
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub(crate) struct IncludeString(pub(crate) String);

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub(crate) struct RegExpString(pub(crate) String);

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub(crate) struct ScopeName(pub(crate) String);

// end/while strings are allowed to refer to captures that occurred in `begin`,
// see https://github.com/shikijs/shiki/issues/918 thus they aren't *really*
// correct regexps
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
pub(crate) struct PartialRegExpString(pub(crate) String);

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum BoolOrNumber {
    Bool(bool),
    Number(u8),
}

fn bool_or_number<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    let bool_or_number = Option::<BoolOrNumber>::deserialize(deserializer)?;
    match bool_or_number {
        None => Ok(None),
        Some(BoolOrNumber::Bool(b)) => Ok(Some(b)),
        Some(BoolOrNumber::Number(0)) => Ok(Some(false)),
        Some(BoolOrNumber::Number(1)) => Ok(Some(true)),
        Some(BoolOrNumber::Number(x)) => Err(serde::de::Error::custom(format!(
            "expected a bool, 0, or 1, got {x}"
        ))),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Rule {
    pub(crate) include: Option<IncludeString>,

    pub(crate) name: Option<ScopeName>,
    pub(crate) content_name: Option<ScopeName>,

    #[serde(rename(deserialize = "match"))]
    pub(crate) match_: Option<RegExpString>,
    pub(crate) captures: Option<Captures>,

    pub(crate) begin: Option<RegExpString>,
    pub(crate) begin_captures: Option<Captures>,

    pub(crate) end: Option<PartialRegExpString>,
    pub(crate) end_captures: Option<Captures>,

    #[serde(rename(deserialize = "while"))]
    pub(crate) while_: Option<PartialRegExpString>,
    pub(crate) while_captures: Option<Captures>,

    pub(crate) patterns: Option<Vec<Rule>>,

    pub(crate) repository: Option<Repository>,

    #[serde(deserialize_with = "bool_or_number")]
    pub(crate) apply_end_pattern_last: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Captures(pub(crate) HashMap<String, Rule>);

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::{Report, Whatever};
    use std::path::Path;
    use test_case::test_case;

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

                SyntaxDefinition::from_json(&json)
                    .with_whatever_context(|_| format!("parsing {} failed", path.display()))?;
            }

            Ok(())
        })
    }
}
