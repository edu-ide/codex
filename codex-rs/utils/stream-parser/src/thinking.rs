use crate::ExtractedInlineTag;
use crate::InlineHiddenTagParser;
use crate::InlineTagSpec;
use crate::StreamTextChunk;
use crate::StreamTextParser;

const OPEN_TAG: &str = "<think>";
const CLOSE_TAG: &str = "</think>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingTag {
    Think,
}

/// Streaming parser that hides `<think>...</think>` blocks from visible output.
///
/// The hidden content is extracted for callers that want to surface it separately,
/// but many callers will ignore it and simply render `visible_text`.
#[derive(Debug)]
pub struct ThinkingTagParser {
    parser: InlineHiddenTagParser<ThinkingTag>,
}

impl ThinkingTagParser {
    pub fn new() -> Self {
        Self {
            parser: InlineHiddenTagParser::new(vec![InlineTagSpec {
                tag: ThinkingTag::Think,
                open: OPEN_TAG,
                close: CLOSE_TAG,
            }]),
        }
    }
}

impl Default for ThinkingTagParser {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamTextParser for ThinkingTagParser {
    type Extracted = String;

    fn push_str(&mut self, chunk: &str) -> StreamTextChunk<Self::Extracted> {
        map_extracted(self.parser.push_str(chunk))
    }

    fn finish(&mut self) -> StreamTextChunk<Self::Extracted> {
        map_extracted(self.parser.finish())
    }
}

fn map_extracted(
    chunk: StreamTextChunk<ExtractedInlineTag<ThinkingTag>>,
) -> StreamTextChunk<String> {
    StreamTextChunk {
        visible_text: chunk.visible_text,
        extracted: chunk
            .extracted
            .into_iter()
            .map(|entry| entry.content)
            .collect(),
    }
}

pub fn strip_think_blocks(text: &str) -> String {
    let mut parser = ThinkingTagParser::new();
    let mut out = parser.push_str(text).visible_text;
    out.push_str(&parser.finish().visible_text);
    out
}

#[cfg(test)]
mod tests {
    use super::ThinkingTagParser;
    use super::strip_think_blocks;
    use crate::StreamTextChunk;
    use crate::StreamTextParser;
    use pretty_assertions::assert_eq;

    fn collect_chunks<P>(parser: &mut P, chunks: &[&str]) -> StreamTextChunk<P::Extracted>
    where
        P: StreamTextParser,
    {
        let mut all = StreamTextChunk::default();
        for chunk in chunks {
            let next = parser.push_str(chunk);
            all.visible_text.push_str(&next.visible_text);
            all.extracted.extend(next.extracted);
        }
        let tail = parser.finish();
        all.visible_text.push_str(&tail.visible_text);
        all.extracted.extend(tail.extracted);
        all
    }

    #[test]
    fn strips_think_blocks_from_visible_text() {
        let text = "before<think>hidden</think>after";
        assert_eq!(strip_think_blocks(text), "beforeafter");
    }

    #[test]
    fn streams_think_blocks_across_chunk_boundaries() {
        let mut parser = ThinkingTagParser::new();
        let out = collect_chunks(&mut parser, &["before<th", "ink>hidden</thi", "nk>after"]);

        assert_eq!(out.visible_text, "beforeafter");
        assert_eq!(out.extracted, vec!["hidden".to_string()]);
    }

    #[test]
    fn auto_closes_unterminated_think_block_on_finish() {
        let mut parser = ThinkingTagParser::new();
        let out = collect_chunks(&mut parser, &["before<think>hidden"]);

        assert_eq!(out.visible_text, "before");
        assert_eq!(out.extracted, vec!["hidden".to_string()]);
    }
}
