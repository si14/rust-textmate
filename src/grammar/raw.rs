use serde::Deserializer;
use serde_derive::Deserialize;
use snafu::prelude::*;
use std::collections::HashMap;

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("failed to deserialize json at \"{}\"", path))]
    Json {
        path: String,
        #[snafu(source(from(serde_path_to_error::Error<serde_json::Error>, serde_path_to_error::Error::into_inner)))]
        source: serde_json::Error,
    },
}

// modelled after https://github.com/microsoft/vscode-textmate/blob/f03a6a8790af81372d0e81facae75554ec5e97ef/src/rawGrammar.ts

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Grammar {
    repository: Option<RepositoryMap>,

    scope_name: String,

    patterns: Vec<Rule>,

    injections: Option<HashMap<String, Rule>>,
    injection_selector: Option<String>,

    inject_to: Option<Vec<String>>,

    file_types: Option<Vec<String>>,
    name: Option<String>,
    first_line_match: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
struct RepositoryMap(HashMap<String, Rule>);

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
struct IncludeString(String);

#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
struct RegExpString(String);

// end/while strings are allowed to refer to captures that occurred in `begin`,
// see https://github.com/shikijs/shiki/issues/918 thus they aren't *really*
// correct regexps
#[derive(Debug, Clone, Deserialize)]
#[serde(transparent)]
struct PartialRegExpString(String);

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum BoolOrNumber {
    Bool(bool),
    Number(u8),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Rule {
    include: Option<IncludeString>,

    name: Option<String>,
    content_name: Option<String>,

    #[serde(rename(deserialize = "match"))]
    match_: Option<RegExpString>,
    captures: Option<Captures>,

    begin: Option<RegExpString>,
    begin_captures: Option<Captures>,

    end: Option<PartialRegExpString>,
    end_captures: Option<Captures>,

    #[serde(rename(deserialize = "while"))]
    while_: Option<PartialRegExpString>,
    while_captures: Option<Captures>,

    patterns: Option<Vec<Rule>>,

    repository: Option<HashMap<String, Rule>>,

    apply_end_pattern_last: Option<BoolOrNumber>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Captures {
    Named(HashMap<String, Rule>),
    // jinja.json uses an array instead; it's semantically clear enough,
    // so let's support it too
    Indexed(Vec<Rule>),
}

impl Grammar {
    pub fn load_from_json(json: &str) -> Result<Grammar, Error> {
        let des = &mut serde_json::Deserializer::from_str(json);

        serde_path_to_error::deserialize(des).with_context(|e| JsonSnafu {
            path: e.path().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snafu::{Report, Whatever};
    use std::path::Path;
    use test_case::test_case;

    const RAW_GRAMMARS_PATH: &str = "tests/textmate-grammars-themes/packages/tm-grammars/raw";
    const CLEANED_GRAMMARS_PATH: &str =
        "tests/textmate-grammars-themes/packages/tm-grammars/grammars";
    const PROBLEMATIC_GRAMMARS: [&str; 4] =
        ["wikitext.json", "stata.json", "racket.json", "xml.json"];

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
                Grammar::load_from_json(&json)
                    .with_whatever_context(|_| format!("loading {} failed", path.display()))?;
            }

            Ok(())
        })
    }
}
