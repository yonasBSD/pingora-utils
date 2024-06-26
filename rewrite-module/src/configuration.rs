// Copyright 2024 Wladimir Palant
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Structures required to deserialize Rewrite Module configuration from YAML configuration files.

use pandora_module_utils::merger::PathMatcher;
use pandora_module_utils::{DeserializeMap, OneOrMany};
use regex::Regex;
use serde::Deserialize;
use std::default::Default;

#[derive(Debug, Clone, PartialEq, Eq)]
enum VariableInterpolationPart {
    Literal(Vec<u8>),
    Variable(String),
}

/// Parsed representation of a string with variable interpolation like the `to` field of the
/// rewrite rule
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub struct VariableInterpolation {
    parts: Vec<VariableInterpolationPart>,
}

impl From<&str> for VariableInterpolation {
    fn from(mut value: &str) -> Self {
        trait FindAt {
            fn find_at(&self, pattern: &str, start: usize) -> Option<usize>;
        }
        impl FindAt for str {
            fn find_at(&self, pattern: &str, start: usize) -> Option<usize> {
                self[start..].find(pattern).map(|index| index + start)
            }
        }

        let mut parts = Vec::new();
        while !value.is_empty() {
            let mut search_start = 0;
            loop {
                let variable_start = value.find_at(Self::VARIABLE_PREFIX, search_start);
                let variable_end =
                    variable_start.and_then(|start| value.find_at(Self::VARIABLE_SUFFIX, start));

                if let (Some(start), Some(end)) = (variable_start, variable_end) {
                    // Found variable start and end, check whether name is alphanumeric
                    let name = &value[start + Self::VARIABLE_PREFIX.len()..end];
                    if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        if start > 0 {
                            parts.push(VariableInterpolationPart::Literal(
                                value[0..start].as_bytes().to_vec(),
                            ));
                        }
                        parts.push(VariableInterpolationPart::Variable(name.to_owned()));
                        value = &value[end + Self::VARIABLE_SUFFIX.len()..];
                        break;
                    }

                    // This variable name is invalid, look for another variable start further ahead
                    search_start = start + Self::VARIABLE_PREFIX.len();
                } else {
                    // No variable found, take the entire value as literal
                    parts.push(VariableInterpolationPart::Literal(
                        value.as_bytes().to_vec(),
                    ));
                    value = "";
                    break;
                }
            }
        }
        Self { parts }
    }
}

impl From<String> for VariableInterpolation {
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

impl VariableInterpolation {
    const VARIABLE_PREFIX: &'static str = "${";
    const VARIABLE_SUFFIX: &'static str = "}";

    pub(crate) fn interpolate<'a, L>(&self, lookup: L) -> Vec<u8>
    where
        L: Fn(&str) -> Option<&'a [u8]>,
    {
        let mut result = Vec::new();
        for part in &self.parts {
            match &part {
                VariableInterpolationPart::Literal(value) => result.extend_from_slice(value),
                VariableInterpolationPart::Variable(name) => {
                    if let Some(value) = lookup(name) {
                        result.extend_from_slice(value);
                    } else {
                        result.extend_from_slice(Self::VARIABLE_PREFIX.as_bytes());
                        result.extend_from_slice(name.as_bytes());
                        result.extend_from_slice(Self::VARIABLE_SUFFIX.as_bytes());
                    }
                }
            }
        }
        result
    }
}

/// URI rewriting type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RewriteType {
    /// An internal rewrite, URI change for internal processing only
    Internal,
    /// A 307 Temporary Redirect response
    Redirect,
    /// A 308 Permanent Redirect response
    Permanent,
}

/// A parsed representation of a field like `from_regex` of the rewrite rule
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "String")]
pub struct RegexMatch {
    /// Regular expression to apply to the value
    pub regex: Regex,
    /// If `true`, the result should be negated
    pub negate: bool,
}

impl RegexMatch {
    /// Checks whether the given value is matched
    pub(crate) fn matches(&self, value: &str) -> bool {
        let result = self.regex.is_match(value);
        if self.negate {
            !result
        } else {
            result
        }
    }
}

impl PartialEq for RegexMatch {
    fn eq(&self, other: &Self) -> bool {
        self.regex.as_str() == other.regex.as_str() && self.negate == other.negate
    }
}

impl Eq for RegexMatch {}

impl TryFrom<&str> for RegexMatch {
    type Error = regex::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let (regex, negate) = if let Some(regex) = value.strip_prefix('!') {
            (regex, true)
        } else {
            (value, false)
        };
        Ok(Self {
            regex: Regex::new(regex)?,
            negate,
        })
    }
}

impl TryFrom<String> for RegexMatch {
    type Error = regex::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.as_str().try_into()
    }
}

/// A rewrite rule resulting in either request URI change or redirect
#[derive(Debug, Clone, PartialEq, Eq, DeserializeMap)]
pub struct RewriteRule {
    /// Path or a set of paths to rewrite
    ///
    /// By default, an exact path match is required. A value like `/path/*` indicates a prefix
    /// match, both `/path/` and `/path/subdir/file.txt` will be matched.
    ///
    /// When multiple rules potentially apply to a location, the closest matches will be evaluated
    /// first. Rules with a longer path are considered closer matches than shorter paths. Exact
    /// matches are considered closer matches than prefix matches for the same path.
    pub from: PathMatcher,

    /// Additional regular expression to further restrict matching paths, e.g. `\.png$` to match
    /// only PNG files. Prefixing the regular expression with `!` will negate its effect, e.g.
    /// `!\.png` will match all files but PNG files.
    ///
    /// Note that restricting the path as much as possible via `from` setting first is recommended
    /// for reasons of performance.
    pub from_regex: Option<RegexMatch>,

    /// Additional regular expression to restrict matches to particular query strings only. For
    /// example `file=` will only match queries containing a `file` parameter. Prefixing the
    /// regular expression with `!` will negate its effect, e.g. `!file=` will match all queries
    /// but those containing a `file` parameter.
    pub query_regex: Option<RegexMatch>,

    /// New URI to be set on match
    ///
    /// The following variables will be resolved:
    ///
    /// * `${tail}`: Only valid when matching a path prefix. This will be replaced by the path part
    ///   matched by `*`. For example, if `from` is `/dir/*`, `to` is `/another/${tail}` and the
    ///   actual path matched is `/dir/file.txt`, then the URI will be rewritten into
    ///   `/another/file.txt`.
    /// * `${query}`: This allows considering the original query which is removed by default. For
    ///   example, if `from` is `/file.txt` and `to` is `/file.html?${query}` then a request to
    ///   `/file.txt?a=b` will be rewritten into `/file.html?a=b`.
    /// * `${http_<header>}`: This allows inserting arbitrary HTTP headers into the redirect
    ///   target.
    pub to: VariableInterpolation,

    /// Rewriting type, one of `internal` (default), `redirect` or `permanent`
    pub r#type: RewriteType,
}

impl Default for RewriteRule {
    fn default() -> Self {
        Self {
            from: "/*".into(),
            from_regex: None,
            query_regex: None,
            to: "/".into(),
            r#type: RewriteType::Internal,
        }
    }
}

/// Configuration file settings of the rewrite module
#[derive(Debug, Default, Clone, PartialEq, Eq, DeserializeMap)]
pub struct RewriteConf {
    /// A list of rewrite rules
    pub rewrite_rules: OneOrMany<RewriteRule>,
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_log::test;

    #[test]
    fn variable_interpolation() {
        assert_eq!(
            VariableInterpolation::from("abcd").interpolate(|_| panic!("Unexpected lookup call")),
            b"abcd".to_vec()
        );

        assert_eq!(
            VariableInterpolation::from("ab${xyz}cd").interpolate(|_| None),
            b"ab${xyz}cd".to_vec()
        );

        assert_eq!(
            VariableInterpolation::from("ab${xyz}cd").interpolate(|name| {
                if name == "xyz" {
                    Some(b"resolved")
                } else {
                    None
                }
            }),
            b"abresolvedcd".to_vec()
        );

        assert_eq!(
            VariableInterpolation::from("a${x}${y}bc${z}d").interpolate(|name| {
                if name == "x" {
                    Some(b"x resolved")
                } else if name == "z" {
                    Some(b"z resolved")
                } else {
                    None
                }
            }),
            b"ax resolved${y}bcz resolvedd".to_vec()
        );

        assert_eq!(
            VariableInterpolation::from("${a${x}").interpolate(|name| {
                if name == "x" {
                    Some(b"resolved")
                } else {
                    None
                }
            }),
            b"${aresolved".to_vec()
        );
    }

    #[test]
    fn regex_match() {
        let regex_match = RegexMatch::try_from("abc").unwrap();
        assert!(regex_match.matches("abc"));
        assert!(regex_match.matches("aabcc"));
        assert!(!regex_match.matches("ab"));
        assert!(!regex_match.matches("bc"));

        let regex_match = RegexMatch::try_from("^abc$").unwrap();
        assert!(regex_match.matches("abc"));
        assert!(!regex_match.matches("aabcc"));
        assert!(!regex_match.matches("ab"));
        assert!(!regex_match.matches("bc"));

        let regex_match = RegexMatch::try_from("!abc").unwrap();
        assert!(!regex_match.matches("abc"));
        assert!(!regex_match.matches("aabcc"));
        assert!(regex_match.matches("ab"));
        assert!(regex_match.matches("bc"));

        let regex_match = RegexMatch::try_from("!^abc$").unwrap();
        assert!(!regex_match.matches("abc"));
        assert!(regex_match.matches("aabcc"));
        assert!(regex_match.matches("ab"));
        assert!(regex_match.matches("bc"));
    }
}
