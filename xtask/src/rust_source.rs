//! A small, dependency-free Rust source scanner for repository tooling.
//!
//! It is not a parser. It classifies every byte as code, comment or literal
//! content so that structural searches (attributes, braces, `mod`
//! declarations, macro arguments) are not confused by strings or comments.
//! The structural view keeps byte offsets identical to the original text.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ByteKind {
    Code,
    Comment,
    /// The contents of a string, byte string, raw string or char literal.
    /// Delimiters (`"`, `'`, `r#"`) remain code.
    Literal,
}

/// Classification of one physical line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineKind {
    Code,
    Comment,
    Blank,
}

pub(crate) struct RustSource {
    text: String,
    kinds: Vec<ByteKind>,
    structural: String,
    line_starts: Vec<usize>,
}

impl RustSource {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let kinds = classify(text.as_bytes());
        // Every non-code or non-ASCII byte becomes one ASCII space, so the
        // structural view is valid UTF-8 with unchanged byte offsets.
        let structural = text
            .bytes()
            .zip(&kinds)
            .map(|(byte, kind)| match kind {
                ByteKind::Code if byte.is_ascii() => byte,
                _ if byte == b'\n' => b'\n',
                _ => b' ',
            })
            .collect::<Vec<_>>();
        let structural = String::from_utf8(structural).expect("structural view is ASCII");
        let mut line_starts = vec![0];
        line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index + 1),
        );
        if line_starts.last() == Some(&text.len()) && !text.is_empty() {
            line_starts.pop();
        }
        Self {
            text,
            kinds,
            structural,
            line_starts,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The source with comments and literal contents replaced by spaces.
    pub(crate) fn structural(&self) -> &str {
        &self.structural
    }

    pub(crate) fn line_count(&self) -> usize {
        if self.text.is_empty() {
            0
        } else {
            self.line_starts.len()
        }
    }

    /// Zero-based line containing byte `offset`.
    pub(crate) fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next - 1,
        }
    }

    pub(crate) fn line_range(&self, line: usize) -> Range<usize> {
        let start = self.line_starts[line];
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());
        start..end
    }

    pub(crate) fn line_kind(&self, line: usize) -> LineKind {
        let range = self.line_range(line);
        let bytes = &self.text.as_bytes()[range.clone()];
        let kinds = &self.kinds[range];
        let mut comment = false;
        for (byte, kind) in bytes.iter().zip(kinds) {
            if byte.is_ascii_whitespace() {
                continue;
            }
            match kind {
                ByteKind::Code | ByteKind::Literal => return LineKind::Code,
                ByteKind::Comment => comment = true,
            }
        }
        if comment {
            LineKind::Comment
        } else {
            LineKind::Blank
        }
    }

    /// Offset of the bracket that closes the one at `open` (`(`, `[` or `{`).
    pub(crate) fn matching_close(&self, open: usize) -> Option<usize> {
        let bytes = self.structural.as_bytes();
        let (open_byte, close_byte) = match bytes.get(open)? {
            b'(' => (b'(', b')'),
            b'[' => (b'[', b']'),
            b'{' => (b'{', b'}'),
            _ => return None,
        };
        let mut depth = 0usize;
        for (index, byte) in bytes.iter().enumerate().skip(open) {
            if *byte == open_byte {
                depth += 1;
            } else if *byte == close_byte {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
        None
    }

    /// Every outer attribute group followed by the item it annotates.
    pub(crate) fn attributed_items(&self) -> Vec<AttributedItem> {
        let bytes = self.structural.as_bytes();
        let mut items = Vec::new();
        let mut index = 0;
        while index + 1 < bytes.len() {
            if bytes[index] == b'#' && bytes[index + 1] == b'[' && self.is_item_start(index) {
                let start = index;
                let mut attributes = Vec::new();
                let mut cursor = index;
                while cursor + 1 < bytes.len() && bytes[cursor] == b'#' && bytes[cursor + 1] == b'['
                {
                    let Some(close) = self.matching_close(cursor + 1) else {
                        return items;
                    };
                    attributes.push(cursor + 2..close);
                    cursor = skip_whitespace(bytes, close + 1);
                }
                let item_start = cursor;
                let item_end = self.item_end(item_start);
                items.push(AttributedItem {
                    start,
                    attributes,
                    item: item_start..item_end,
                });
                index = cursor;
            } else {
                index += 1;
            }
        }
        items
    }

    fn is_item_start(&self, index: usize) -> bool {
        let before = self.structural.as_bytes()[..index]
            .iter()
            .rev()
            .find(|byte| !byte.is_ascii_whitespace());
        matches!(before, None | Some(b';' | b'}' | b'{' | b']' | b','))
    }

    /// The end of the item, statement, field, variant or match arm starting
    /// at `start`.
    ///
    /// Items end one past their closing `}` or terminating `;`. Fields,
    /// variants and arms end after their `,` or before the `}`/`)` that
    /// closes the enclosing list.
    pub(crate) fn item_end(&self, start: usize) -> usize {
        let bytes = self.structural.as_bytes();
        if self.starts_item(start) {
            let mut depth = 0isize;
            for (index, byte) in bytes.iter().enumerate().skip(start) {
                match byte {
                    b'(' | b'[' => depth += 1,
                    b')' | b']' => depth -= 1,
                    b';' if depth <= 0 => return index + 1,
                    b'{' if depth <= 0 => {
                        return self
                            .matching_close(index)
                            .map_or(bytes.len(), |close| close + 1);
                    }
                    _ => {}
                }
            }
            return bytes.len();
        }
        let mut depth = 0isize;
        for (index, byte) in bytes.iter().enumerate().skip(start) {
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' if depth == 0 => return index,
                b')' | b']' | b'}' => depth -= 1,
                b',' if depth == 0 => return index + 1,
                _ => {}
            }
        }
        bytes.len()
    }

    /// Whether `start` begins an item or `let` statement rather than a
    /// field, variant or match arm.
    fn starts_item(&self, start: usize) -> bool {
        const ITEM_KEYWORDS: [&str; 17] = [
            "fn",
            "struct",
            "enum",
            "union",
            "mod",
            "impl",
            "trait",
            "use",
            "const",
            "static",
            "type",
            "extern",
            "unsafe",
            "async",
            "let",
            "macro_rules",
            "default",
        ];
        let bytes = self.structural.as_bytes();
        let mut cursor = start;
        loop {
            let word_end = identifier_end(bytes, cursor);
            let word = &self.structural[cursor..word_end];
            if word == "pub" {
                cursor = skip_whitespace(bytes, word_end);
                if bytes.get(cursor) == Some(&b'(') {
                    let Some(close) = self.matching_close(cursor) else {
                        return true;
                    };
                    cursor = skip_whitespace(bytes, close + 1);
                }
                continue;
            }
            let next = skip_whitespace(bytes, word_end);
            return ITEM_KEYWORDS.contains(&word) || bytes.get(next) == Some(&b'!');
        }
    }

    /// `mod name;` and `mod name { ... }` declarations with their attributes.
    pub(crate) fn module_declarations(&self) -> Vec<ModuleDeclaration> {
        let attributed = self.attributed_items();
        let bytes = self.structural.as_bytes();
        let mut declarations = Vec::new();
        for (keyword, _) in self.structural.match_indices("mod") {
            if !is_word_at(bytes, keyword, 3) || !self.is_module_keyword_position(keyword) {
                continue;
            }
            let name_start = skip_whitespace(bytes, keyword + 3);
            let name_end = identifier_end(bytes, name_start);
            if name_end == name_start {
                continue;
            }
            let after = skip_whitespace(bytes, name_end);
            let (inline_body, end) = match bytes.get(after) {
                Some(b';') => (None, after + 1),
                Some(b'{') => {
                    let close = self.matching_close(after).unwrap_or(bytes.len());
                    (Some(after + 1..close), close + 1)
                }
                _ => continue,
            };
            let attributes = attributed
                .iter()
                .find(|item| item.item.start <= keyword && keyword < item.item.end)
                .filter(|item| {
                    self.structural[item.item.start..keyword]
                        .split_whitespace()
                        .all(is_visibility_token)
                })
                .map(|item| item.attributes.clone())
                .unwrap_or_default();
            let name = self.structural[name_start..name_end]
                .trim_start_matches("r#")
                .to_owned();
            declarations.push(ModuleDeclaration {
                name,
                start: keyword,
                end,
                attributes,
                inline_body,
            });
        }
        declarations
    }

    fn is_module_keyword_position(&self, keyword: usize) -> bool {
        let bytes = self.structural.as_bytes();
        // Walk back over an optional visibility qualifier.
        let mut cursor = keyword;
        loop {
            let previous = self.structural[..cursor].trim_end();
            if let Some(stripped) = previous.strip_suffix(')') {
                if let Some(open) = stripped.rfind("pub(") {
                    cursor = open;
                    continue;
                }
                return false;
            }
            if previous.ends_with("pub") && is_word_at(bytes, previous.len() - 3, 3) {
                cursor = previous.len() - 3;
                continue;
            }
            return matches!(
                previous.as_bytes().last(),
                None | Some(b';' | b'}' | b'{' | b']')
            );
        }
    }

    /// The text of an attribute range, e.g. `cfg(test)`.
    pub(crate) fn attribute_text(&self, range: &Range<usize>) -> &str {
        &self.structural[range.clone()]
    }

    /// The literal value of `#[path = "..."]`, read from the original text.
    pub(crate) fn path_attribute(&self, range: &Range<usize>) -> Option<String> {
        let structural = self.structural[range.clone()].trim();
        let rest = structural.strip_prefix("path")?.trim_start();
        rest.strip_prefix('=')?;
        let original = &self.text[range.clone()];
        let open = original.find('"')?;
        let close = original[open + 1..].find('"')? + open + 1;
        Some(original[open + 1..close].to_owned())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AttributedItem {
    /// Offset of the first `#` of the attribute group.
    pub(crate) start: usize,
    /// Contents of each `#[...]`, without the brackets.
    pub(crate) attributes: Vec<Range<usize>>,
    pub(crate) item: Range<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct ModuleDeclaration {
    pub(crate) name: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) attributes: Vec<Range<usize>>,
    /// The body of an inline `mod name { ... }`.
    pub(crate) inline_body: Option<Range<usize>>,
}

/// Whether an attribute (its contents without `#[` `]`) restricts the item to
/// test builds: `test`, `cfg(test)` or `cfg(all(.., test, ..))`.
pub(crate) fn is_test_only_attribute(attribute: &str) -> bool {
    let attribute = attribute.trim();
    if attribute == "test" {
        return true;
    }
    let Some(predicate) = attribute
        .strip_prefix("cfg")
        .map(str::trim_start)
        .and_then(|rest| rest.strip_prefix('('))
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    cfg_requires_test(predicate.trim())
}

fn cfg_requires_test(predicate: &str) -> bool {
    if predicate == "test" {
        return true;
    }
    let call = |name: &str| {
        predicate
            .strip_prefix(name)
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix('('))
            .and_then(|rest| rest.strip_suffix(')'))
            .map(split_top_level)
    };
    if let Some(arguments) = call("all") {
        return arguments.iter().any(|argument| cfg_requires_test(argument));
    }
    if let Some(arguments) = call("any") {
        return !arguments.is_empty()
            && arguments.iter().all(|argument| cfg_requires_test(argument));
    }
    false
}

fn split_top_level(arguments: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (index, character) in arguments.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(arguments[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let last = arguments[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    parts
}

/// `pub`, `pub(crate)` and the pieces of `pub(in path)` split on whitespace.
fn is_visibility_token(token: &str) -> bool {
    token == "pub" || token.starts_with("pub(") || token.ends_with(')')
}

fn skip_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub(crate) fn identifier_end(bytes: &[u8], start: usize) -> usize {
    let mut index = start;
    if bytes[index..].starts_with(b"r#") {
        index += 2;
    }
    while index < bytes.len() && is_identifier_byte(bytes[index]) {
        index += 1;
    }
    if index == start + 2 && bytes[start..].starts_with(b"r#") {
        return start;
    }
    index
}

/// Whether the `len` bytes at `start` form a whole word.
pub(crate) fn is_word_at(bytes: &[u8], start: usize, len: usize) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|index| bytes.get(index))
        .is_some_and(|byte| is_identifier_byte(*byte));
    let after = bytes
        .get(start + len)
        .is_some_and(|byte| is_identifier_byte(*byte));
    !before && !after
}

fn classify(bytes: &[u8]) -> Vec<ByteKind> {
    let mut kinds = vec![ByteKind::Code; bytes.len()];
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let next = bytes.get(index + 1).copied();
        if byte == b'/' && next == Some(b'/') {
            let end = bytes[index..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| index + offset);
            kinds[index..end].fill(ByteKind::Comment);
            index = end;
        } else if byte == b'/' && next == Some(b'*') {
            let end = block_comment_end(bytes, index);
            kinds[index..end].fill(ByteKind::Comment);
            index = end;
        } else if let Some(hashes) = raw_string_start(bytes, index) {
            // r"..." / r#"..."# / br"..." / cr"..."
            let quote = index + bytes[index..].iter().position(|b| *b == b'"').unwrap_or(0);
            let content_start = quote + 1;
            let mut terminator = vec![b'"'];
            terminator.extend(std::iter::repeat_n(b'#', hashes));
            let content_end = bytes[content_start..]
                .windows(terminator.len())
                .position(|window| window == terminator.as_slice())
                .map_or(bytes.len(), |offset| content_start + offset);
            kinds[content_start..content_end].fill(ByteKind::Literal);
            index = (content_end + terminator.len()).min(bytes.len());
        } else if byte == b'"' {
            // Also covers b"..." and c"...": the prefix letter stays code.
            index = mask_quoted(bytes, &mut kinds, index, b'"');
        } else if byte == b'\'' && is_char_literal(bytes, index) {
            index = mask_quoted(bytes, &mut kinds, index, b'\'');
        } else {
            index += 1;
        }
    }
    kinds
}

/// Number of `#` if a raw string literal starts at `index`.
fn raw_string_start(bytes: &[u8], index: usize) -> Option<usize> {
    if index > 0 && is_identifier_byte(bytes[index - 1]) {
        return None;
    }
    let mut cursor = index;
    if matches!(bytes.get(cursor), Some(b'b' | b'c')) {
        cursor += 1;
    }
    if bytes.get(cursor) != Some(&b'r') {
        return None;
    }
    cursor += 1;
    let mut hashes = 0;
    while bytes.get(cursor) == Some(&b'#') {
        hashes += 1;
        cursor += 1;
    }
    (bytes.get(cursor) == Some(&b'"')).then_some(hashes)
}

fn block_comment_end(bytes: &[u8], start: usize) -> usize {
    let mut depth = 0usize;
    let mut index = start;
    while index + 1 < bytes.len() {
        if bytes[index] == b'/' && bytes[index + 1] == b'*' {
            depth += 1;
            index += 2;
        } else if bytes[index] == b'*' && bytes[index + 1] == b'/' {
            depth -= 1;
            index += 2;
            if depth == 0 {
                return index;
            }
        } else {
            index += 1;
        }
    }
    bytes.len()
}

/// Distinguishes `'x'`, `'\n'` and `'\u{..}'` from lifetimes and labels.
fn is_char_literal(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index + 1) {
        Some(b'\\') => true,
        Some(_) => {
            // A char literal closes after one (possibly multi-byte) character.
            let start = index + 1;
            let width = utf8_width(bytes[start]);
            bytes.get(start + width) == Some(&b'\'')
        }
        None => false,
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

fn mask_quoted(bytes: &[u8], kinds: &mut [ByteKind], open: usize, quote: u8) -> usize {
    let mut index = open + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                kinds[index] = ByteKind::Literal;
                if index + 1 < bytes.len() {
                    kinds[index + 1] = ByteKind::Literal;
                }
                index += 2;
            }
            byte if byte == quote => return index + 1,
            _ => {
                kinds[index] = ByteKind::Literal;
                index += 1;
            }
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_comments_do_not_affect_structure() {
        let source = RustSource::new(
            "fn a() { let s = \"}{\"; let c = '}'; // }\n /* { */ let r = r#\"}\"#; }\nfn b<'a>() {}\n",
        );
        let open = source.structural().find('{').unwrap();
        let close = source.matching_close(open).unwrap();
        assert_eq!(source.line_of(close), 1);
        assert!(!source.structural().contains("//"));
        assert!(source.structural().contains("fn b<'a>()"));
    }

    #[test]
    fn line_kinds_distinguish_code_comments_and_blanks() {
        let source = RustSource::new("// doc\n\nlet x = \"\n\n\";\n/* a\n b */\n");
        let kinds = (0..source.line_count())
            .map(|line| source.line_kind(line))
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            [
                LineKind::Comment,
                LineKind::Blank,
                LineKind::Code,
                // A blank line inside a string literal is still blank text.
                LineKind::Blank,
                LineKind::Code,
                LineKind::Comment,
                LineKind::Comment,
            ]
        );
    }

    #[test]
    fn test_only_attributes_are_recognized() {
        for attribute in [
            "test",
            "cfg(test)",
            "cfg(all(test, not(target_os = \"android\")))",
            "cfg(any(test, all(test, unix)))",
        ] {
            assert!(is_test_only_attribute(attribute), "{attribute}");
        }
        for attribute in [
            "cfg(not(test))",
            "cfg(any(test, feature = \"x\"))",
            "cfg(target_os = \"android\")",
            "derive(Debug)",
        ] {
            assert!(!is_test_only_attribute(attribute), "{attribute}");
        }
    }

    #[test]
    fn module_declarations_carry_attributes_and_paths() {
        let source = RustSource::new(
            "mod a;\n#[cfg(test)]\nmod tests;\n#[path = \"x/y.rs\"]\npub(crate) mod z;\nmod inline { mod nested; }\nfn module() {}\n",
        );
        let declarations = source.module_declarations();
        let names = declarations
            .iter()
            .map(|declaration| declaration.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["a", "tests", "z", "inline", "nested"]);
        assert!(is_test_only_attribute(
            source.attribute_text(&declarations[1].attributes[0])
        ));
        assert_eq!(
            source
                .path_attribute(&declarations[2].attributes[0])
                .as_deref(),
            Some("x/y.rs")
        );
        assert!(declarations[3].inline_body.is_some());
    }

    #[test]
    fn attributed_items_span_their_blocks() {
        let source = RustSource::new(
            "#[cfg(test)]\nmod tests {\n    fn x(a: [u8; 2]) {}\n}\nfn after() {}\n",
        );
        let items = source.attributed_items();
        assert_eq!(items.len(), 1);
        assert_eq!(source.line_of(items[0].item.end - 1), 3);
    }
}
