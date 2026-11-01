use crate::filters::Filter;
use crate::index::Index;
use anyhow::{Result, bail, ensure};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct QueryOptions {
    pub case_sensitive: bool,
    pub path_match: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Debug)]
pub struct Query {
    node: Node,
    case_sensitive: bool,
    filter: Filter,
}
#[derive(Debug)]
enum Node {
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    Name(Needle),
    Path(Needle),
    Extension(Vec<Vec<u8>>),
    Directory(bool),
    Filter(Filter),
}
#[derive(Debug)]
enum Needle {
    Literal(Vec<u8>),
    Pattern(Regex),
}
impl Needle {
    fn new(text: &str, case_sensitive: bool, regex: bool, whole_word: bool) -> Result<Self> {
        if regex || whole_word || text.contains(['*', '?']) {
            let expression = if regex {
                text.to_owned()
            } else if text.contains(['*', '?']) {
                let mut expression = String::from("^");
                for c in text.chars() {
                    match c {
                        '*' => expression.push_str(".*"),
                        '?' => expression.push('.'),
                        c => expression.push_str(&regex::escape(&c.to_string())),
                    }
                }
                expression.push('$');
                expression
            } else {
                regex::escape(text)
            };
            let expression = if whole_word {
                format!(r"\b(?:{expression})\b")
            } else {
                expression
            };
            Ok(Self::Pattern(
                RegexBuilder::new(&expression)
                    .case_insensitive(!case_sensitive)
                    .size_limit(1024 * 1024)
                    .build()?,
            ))
        } else {
            Ok(Self::Literal(
                if case_sensitive {
                    text.to_owned()
                } else {
                    text.to_lowercase()
                }
                .into_bytes(),
            ))
        }
    }
    fn matches(&self, bytes: &[u8]) -> bool {
        match self {
            Self::Literal(needle) => memchr::memmem::find(bytes, needle).is_some(),
            Self::Pattern(pattern) => std::str::from_utf8(bytes).is_ok_and(|s| pattern.is_match(s)),
        }
    }
}
impl Query {
    /// Whitespace means AND. Pipe means OR. Quotes preserve spaces; ! negates a term/group.
    pub fn parse(text: &str, case_sensitive: bool) -> Result<Self> {
        Self::parse_with(
            text,
            QueryOptions {
                case_sensitive,
                ..Default::default()
            },
            Filter::All,
        )
    }
    pub fn parse_with(text: &str, options: QueryOptions, filter: Filter) -> Result<Self> {
        ensure!(text.len() <= 8192, "Query exceeds 8 KB");
        if options.regex && !text.is_empty() {
            let needle = Needle::new(text, options.case_sensitive, true, options.whole_word)?;
            return Ok(Self {
                node: if options.path_match {
                    Node::Path(needle)
                } else {
                    Node::Name(needle)
                },
                case_sensitive: options.case_sensitive,
                filter,
            });
        }
        let mut tokens = Vec::new();
        let mut word = String::new();
        let mut quoted = false;
        let flush = |word: &mut String, tokens: &mut Vec<String>| {
            if !word.is_empty() {
                tokens.push(std::mem::take(word));
            }
        };
        for c in text.chars() {
            match c {
                '"' => quoted = !quoted,
                c if !quoted && (c.is_whitespace() || ['|', '(', ')', '!'].contains(&c)) => {
                    flush(&mut word, &mut tokens);
                    if !c.is_whitespace() {
                        tokens.push(c.to_string());
                    }
                }
                c => word.push(c),
            }
        }
        ensure!(!quoted, "Unclosed quote");
        flush(&mut word, &mut tokens);
        ensure!(tokens.len() <= 256, "Query exceeds 256 tokens");
        let mut parser = Parser {
            tokens,
            at: 0,
            options,
        };
        let node = if parser.tokens.is_empty() {
            Node::And(Vec::new())
        } else {
            parser.or(0)?
        };
        ensure!(
            parser.at == parser.tokens.len(),
            "Unexpected token {}",
            parser.tokens.get(parser.at).map_or("", String::as_str)
        );
        Ok(Self {
            node,
            case_sensitive: options.case_sensitive,
            filter,
        })
    }
    pub(crate) fn candidates<'a>(
        &self,
        postings: &'a crate::trigram::Postings,
        cap: usize,
        budget: &mut crate::trigram::PlanBudget,
        epoch: crate::trigram::Epoch<'_>,
    ) -> Result<crate::trigram::Candidates<'a>> {
        if self.case_sensitive {
            return Ok(crate::trigram::Candidates::All);
        }
        self.node.candidates(postings, cap, budget, epoch)
    }
    pub fn matches(&self, index: &Index, id: u32) -> bool {
        self.filter.matches(index, id) && self.node.matches(index, id, self.case_sensitive)
    }
}
impl Node {
    fn candidates<'a>(
        &self,
        postings: &'a crate::trigram::Postings,
        cap: usize,
        budget: &mut crate::trigram::PlanBudget,
        epoch: crate::trigram::Epoch<'_>,
    ) -> Result<crate::trigram::Candidates<'a>> {
        use crate::trigram::Candidates;
        crate::trigram::check(epoch)?;
        Ok(match self {
            Self::Name(Needle::Literal(needle)) => postings.literal(needle, cap),
            Self::And(nodes) => {
                let mut best = Candidates::All;
                for node in nodes {
                    let next = node.candidates(postings, cap, budget, epoch)?;
                    if next.len() < best.len() {
                        best = next;
                    }
                    if best.len() == 0 {
                        break;
                    }
                }
                best
            }
            Self::Or(nodes) => {
                let mut result = Candidates::Ids(std::borrow::Cow::Borrowed(&[]));
                for node in nodes {
                    result = result.union(
                        node.candidates(postings, cap, budget, epoch)?,
                        cap,
                        budget,
                        epoch,
                    )?;
                    if matches!(result, Candidates::All) {
                        break;
                    }
                }
                result
            }
            // NOT, paths, regular expressions and short literals cannot safely
            // restrict the universe. AND may still use another positive term.
            _ => Candidates::All,
        })
    }
    fn matches(&self, i: &Index, id: u32, case_sensitive: bool) -> bool {
        match self {
            Self::And(nodes) => nodes.iter().all(|n| n.matches(i, id, case_sensitive)),
            Self::Or(nodes) => nodes.iter().any(|n| n.matches(i, id, case_sensitive)),
            Self::Not(n) => !n.matches(i, id, case_sensitive),
            Self::Directory(expected) => i.records[id as usize].is_dir() == *expected,
            Self::Filter(filter) => filter.matches(i, id),
            Self::Name(needle) => {
                if case_sensitive || matches!(needle, Needle::Pattern(_)) {
                    needle.matches(i.name(id).as_bytes())
                } else {
                    needle.matches(i.folded_name(id))
                }
            }
            Self::Path(needle) => i.path(id).is_ok_and(|p| {
                let s = p.to_string_lossy();
                if case_sensitive || matches!(needle, Needle::Pattern(_)) {
                    needle.matches(s.as_bytes())
                } else {
                    needle.matches(s.to_lowercase().as_bytes())
                }
            }),
            Self::Extension(extensions) => {
                if i.records[id as usize].is_dir() {
                    return false;
                }
                if case_sensitive {
                    let name = i.name(id);
                    name.rsplit_once('.').is_some_and(|(_, ext)| {
                        extensions.iter().any(|e| e.as_slice() == ext.as_bytes())
                    })
                } else {
                    i.folded_extension(id)
                        .is_some_and(|ext| extensions.iter().any(|e| e.as_slice() == ext))
                }
            }
        }
    }
}
struct Parser {
    tokens: Vec<String>,
    at: usize,
    options: QueryOptions,
}
impl Parser {
    fn peek(&self) -> &str {
        self.tokens.get(self.at).map_or("", String::as_str)
    }
    fn or(&mut self, depth: usize) -> Result<Node> {
        let mut nodes = vec![self.and(depth)?];
        while self.peek() == "|" {
            self.at += 1;
            nodes.push(self.and(depth)?);
        }
        Ok(if nodes.len() == 1 {
            nodes.pop().unwrap()
        } else {
            Node::Or(nodes)
        })
    }
    fn and(&mut self, depth: usize) -> Result<Node> {
        let mut nodes = Vec::new();
        while !["", "|", ")"].contains(&self.peek()) {
            nodes.push(self.atom(depth)?);
        }
        ensure!(!nodes.is_empty(), "Expected search term");
        Ok(if nodes.len() == 1 {
            nodes.pop().unwrap()
        } else {
            Node::And(nodes)
        })
    }
    fn atom(&mut self, depth: usize) -> Result<Node> {
        ensure!(depth <= 32, "Query nesting exceeds 32 levels");
        let token = self.peek().to_owned();
        self.at += 1;
        if let Some(filter) = Filter::ALL
            .into_iter()
            .find(|f| f.keyword() == Some(token.as_str()))
        {
            return Ok(Node::Filter(filter));
        }
        match token.as_str() {
            "!" => Ok(Node::Not(Box::new(self.atom(depth + 1)?))),
            "(" => {
                let n = self.or(depth + 1)?;
                ensure!(self.peek() == ")", "Unclosed group");
                self.at += 1;
                Ok(n)
            }
            "" | "|" | ")" => bail!("Expected term after negation or group"),
            "file:" => Ok(Node::Directory(false)),
            "folder:" => Ok(Node::Directory(true)),
            t if t.starts_with("ext:") => {
                let text = &t[4..];
                ensure!(!text.is_empty(), "ext: requires an extension");
                let mut exts = Vec::new();
                for ext in text.split(';') {
                    let ext = ext.trim_start_matches('.');
                    ensure!(!ext.is_empty(), "Empty extension");
                    exts.push(
                        if self.options.case_sensitive {
                            ext.to_owned()
                        } else {
                            ext.to_lowercase()
                        }
                        .into_bytes(),
                    );
                }
                Ok(Node::Extension(exts))
            }
            t if t.starts_with("path:") => {
                ensure!(t.len() > 5, "path: requires text");
                Ok(Node::Path(Needle::new(
                    &t[5..],
                    self.options.case_sensitive,
                    false,
                    self.options.whole_word,
                )?))
            }
            t if t.starts_with("regex:") => {
                ensure!(t.len() > 6, "regex: requires a pattern");
                let needle = Needle::new(
                    &t[6..],
                    self.options.case_sensitive,
                    true,
                    self.options.whole_word,
                )?;
                Ok(if self.options.path_match {
                    Node::Path(needle)
                } else {
                    Node::Name(needle)
                })
            }
            t => {
                let needle = Needle::new(
                    t,
                    self.options.case_sensitive,
                    false,
                    self.options.whole_word,
                )?;
                let drive =
                    t.len() == 2 && t.as_bytes()[0].is_ascii_alphabetic() && t.ends_with(':');
                Ok(
                    if self.options.path_match || t.contains(['\\', '/']) || drive {
                        Node::Path(needle)
                    } else {
                        Node::Name(needle)
                    },
                )
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{index::Source, usn::UsnRecord};
    fn fixture() -> Index {
        let mut i = Index::new(
            Source::Folder {
                root: "C:\\sample".into(),
            },
            1,
        );
        for (frn, name, attr) in [
            (2, "한국 Report.TXT", 0),
            (3, "notes.rs", 0),
            (4, "Reports", 16),
        ] {
            i.upsert(&UsnRecord {
                frn,
                parent: 1,
                usn: 1,
                reason: 0,
                attributes: attr,
                name: name.encode_utf16().collect(),
            })
            .unwrap();
        }
        i.rebuild_order(None).unwrap();
        i
    }
    #[test]
    fn boolean_filters_unicode_quotes_and_wildcards() {
        let i = fixture();
        for (q, count) in [
            ("한국 report", 1),
            ("\"한국 Report\"", 1),
            ("ext:txt;rs file:", 2),
            ("folder: | ext:rs", 2),
            ("!(folder: | ext:rs)", 1),
            ("*.txt", 1),
            ("path:sample", 3),
            ("regex:\"Report.*TXT$\"", 1),
        ] {
            assert_eq!(
                i.search(&Query::parse(q, false).unwrap(), 10, None)
                    .unwrap()
                    .total,
                count,
                "{q}"
            );
        }
        assert_eq!(
            i.search(&Query::parse("report", true).unwrap(), 10, None)
                .unwrap()
                .total,
            0
        );
    }
    #[test]
    fn malformed_or_excessive_queries_fail() {
        for q in ["!", "a |", "(a", "a)", "\"a", "ext:", "regex:[", "a || b"] {
            assert!(Query::parse(q, false).is_err(), "{q}");
        }
        assert!(Query::parse(&"(".repeat(40), false).is_err());
    }
}

#[cfg(test)]
mod option_tests {
    use super::*;
    use crate::{
        index::{Index, SearchOrder, Source},
        usn::UsnRecord,
    };
    fn fixture() -> Index {
        let mut i = Index::new(
            Source::Folder {
                root: "C:\\sample".into(),
            },
            1,
        );
        for (n, (name, attributes)) in [
            ("alpha.txt", 0),
            ("alphabet.JPG", 0),
            ("zeta", 16),
            ("beta.zip", 0),
        ]
        .into_iter()
        .enumerate()
        {
            i.upsert(&UsnRecord {
                frn: n as u64 + 2,
                parent: 1,
                usn: 0,
                reason: 0,
                attributes,
                name: name.encode_utf16().collect(),
            })
            .unwrap();
        }
        i.rebuild_order(None).unwrap();
        i
    }
    fn count(i: &Index, text: &str, options: QueryOptions, filter: Filter) -> usize {
        i.search(&Query::parse_with(text, options, filter).unwrap(), 10, None)
            .unwrap()
            .total
    }
    #[test]
    fn filters_path_whole_words_and_regex_are_independent() {
        let i = fixture();
        let defaults = QueryOptions::default();
        assert_eq!(count(&i, "alpha", defaults, Filter::All), 2);
        assert_eq!(
            count(
                &i,
                "alpha",
                QueryOptions {
                    whole_word: true,
                    ..defaults
                },
                Filter::All
            ),
            1
        );
        assert_eq!(
            count(
                &i,
                "sample",
                QueryOptions {
                    path_match: true,
                    ..defaults
                },
                Filter::All
            ),
            4
        );
        assert_eq!(count(&i, "C:\\sample", defaults, Filter::All), 4);
        assert_eq!(count(&i, "image:", defaults, Filter::All), 1);
        assert_eq!(count(&i, "", defaults, Filter::Archives), 1);
        assert_eq!(count(&i, "", defaults, Filter::Folders), 1);
        assert_eq!(
            count(
                &i,
                r"^(alpha|beta)\.",
                QueryOptions {
                    regex: true,
                    ..defaults
                },
                Filter::All
            ),
            2
        );
        assert_eq!(count(&i, "image:", defaults, Filter::Files), 1);
        assert_eq!(
            count(
                &i,
                r"(?-i:JPG)",
                QueryOptions {
                    regex: true,
                    ..defaults
                },
                Filter::All
            ),
            1
        );
        assert_eq!(
            count(
                &i,
                r"(?-i:jpg)",
                QueryOptions {
                    regex: true,
                    ..defaults
                },
                Filter::All
            ),
            0
        );
        assert!(
            Query::parse_with(
                "[",
                QueryOptions {
                    regex: true,
                    ..defaults
                },
                Filter::All
            )
            .is_err()
        );
    }
    #[test]
    fn sort_limit_applies_to_whole_index_and_folder_priority() {
        let mut i = fixture();
        // Cross the parallel count block boundary to catch reversing only the retained page.
        for n in 0..5000 {
            i.upsert(&UsnRecord {
                frn: 100 + n,
                parent: 1,
                usn: 0,
                reason: 0,
                attributes: 0,
                name: format!("m{n:05}.txt").encode_utf16().collect(),
            })
            .unwrap();
        }
        i.rebuild_order(None).unwrap();
        let q = Query::parse("", false).unwrap();
        let r = i
            .search_ordered(
                &q,
                2,
                None,
                SearchOrder {
                    column: Default::default(),
                    descending: true,
                    folders_first: false,
                },
            )
            .unwrap();
        assert_eq!(r.total, 5004);
        assert_eq!(i.name(r.ids[0]), "zeta");
        assert_eq!(i.name(r.ids[1]), "m04999.txt");
        let r = i
            .search_ordered(
                &q,
                2,
                None,
                SearchOrder {
                    column: Default::default(),
                    descending: false,
                    folders_first: true,
                },
            )
            .unwrap();
        assert_eq!(i.name(r.ids[0]), "zeta");
        assert_eq!(i.name(r.ids[1]), "alpha.txt");
    }
}
