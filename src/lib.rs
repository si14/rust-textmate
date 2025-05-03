use crate::syntax_compiler::{compile, parse};

pub(crate) mod syntax_compiler;

const ABC_TMLANG: &str = r##"{
  "scopeName": "source.abc",
  "patterns": [
    {
      "include": "#expression"
    }
  ],
  "repository": {
    "expression": {
      "patterns": [
        {
          "include": "#letter"
        },
        {
          "include": "#paren-expression"
        }
      ]
    },
    "letter": {
      "match": "a|b|c",
      "name": "keyword.letter"
    },
    "paren-expression": {
      "begin": "\\(",
      "end": "\\)",
      "beginCaptures": {
        "0": {
          "name": "punctuation.paren.open"
        }
      },
      "endCaptures": {
        "0": {
          "name": "punctuation.paren.close"
        }
      },
      "name": "expression.group",
      "patterns": [
        {
          "include": "#expression"
        }
      ]
    }
  }
}"##;

const ABC_PROGRAM: &str = "\
a
(
    b
)
x
(
    (
        c
        xyz
    )
)
(
a";

fn parse_line(syntax: &compile::SyntaxDefinition, line: &str) {}

pub fn test() {
    let parsed = parse::SyntaxDefinition::from_json(ABC_TMLANG).unwrap();
    let compiled = compile::SyntaxDefinition::compile(parsed).unwrap();

    println!("{:?}", compiled);
}
