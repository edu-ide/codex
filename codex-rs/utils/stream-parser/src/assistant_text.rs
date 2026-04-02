use crate::CitationStreamParser;
use crate::ProposedPlanParser;
use crate::ProposedPlanSegment;
use crate::StreamTextChunk;
use crate::StreamTextParser;
use crate::ThinkingTagParser;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssistantTextChunk {
    pub visible_text: String,
    pub citations: Vec<String>,
    pub plan_segments: Vec<ProposedPlanSegment>,
}

impl AssistantTextChunk {
    pub fn is_empty(&self) -> bool {
        self.visible_text.is_empty() && self.citations.is_empty() && self.plan_segments.is_empty()
    }
}

/// Parses assistant text streaming markup in one pass:
/// - strips `<oai-mem-citation>` tags and extracts citation payloads
/// - in plan mode, also strips `<proposed_plan>` blocks and emits plan segments
#[derive(Debug, Default)]
pub struct AssistantTextStreamParser {
    plan_mode: bool,
    hide_think_tags: bool,
    thinking: ThinkingTagParser,
    citations: CitationStreamParser,
    plan: ProposedPlanParser,
}

impl AssistantTextStreamParser {
    pub fn new(plan_mode: bool, hide_think_tags: bool) -> Self {
        Self {
            plan_mode,
            hide_think_tags,
            ..Self::default()
        }
    }

    pub fn push_str(&mut self, chunk: &str) -> AssistantTextChunk {
        let visible_for_citations = if self.hide_think_tags {
            self.thinking.push_str(chunk).visible_text
        } else {
            chunk.to_string()
        };
        let citation_chunk = self.citations.push_str(&visible_for_citations);
        let mut out = self.parse_visible_text(citation_chunk.visible_text);
        out.citations = citation_chunk.extracted;
        out
    }

    pub fn finish(&mut self) -> AssistantTextChunk {
        let think_tail = if self.hide_think_tags {
            self.thinking.finish().visible_text
        } else {
            String::new()
        };
        let mut citation_chunk = self.citations.push_str(&think_tail);
        let citation_finish = self.citations.finish();
        citation_chunk
            .visible_text
            .push_str(&citation_finish.visible_text);
        citation_chunk.extracted.extend(citation_finish.extracted);
        let mut out = self.parse_visible_text(citation_chunk.visible_text);
        if self.plan_mode {
            let mut tail = self.plan.finish();
            if !tail.is_empty() {
                out.visible_text.push_str(&tail.visible_text);
                out.plan_segments.append(&mut tail.extracted);
            }
        }
        out.citations = citation_chunk.extracted;
        out
    }

    fn parse_visible_text(&mut self, visible_text: String) -> AssistantTextChunk {
        if !self.plan_mode {
            return AssistantTextChunk {
                visible_text,
                ..AssistantTextChunk::default()
            };
        }
        let plan_chunk: StreamTextChunk<ProposedPlanSegment> = self.plan.push_str(&visible_text);
        AssistantTextChunk {
            visible_text: plan_chunk.visible_text,
            plan_segments: plan_chunk.extracted,
            ..AssistantTextChunk::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AssistantTextStreamParser;
    use crate::ProposedPlanSegment;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_citations_across_seed_and_delta_boundaries() {
        let mut parser = AssistantTextStreamParser::new(
            /*plan_mode*/ false, /*hide_think_tags*/ false,
        );

        let seeded = parser.push_str("hello <oai-mem-citation>doc");
        let parsed = parser.push_str("1</oai-mem-citation> world");
        let tail = parser.finish();

        assert_eq!(seeded.visible_text, "hello ");
        assert_eq!(seeded.citations, Vec::<String>::new());
        assert_eq!(parsed.visible_text, " world");
        assert_eq!(parsed.citations, vec!["doc1".to_string()]);
        assert_eq!(tail.visible_text, "");
        assert_eq!(tail.citations, Vec::<String>::new());
    }

    #[test]
    fn parses_plan_segments_after_citation_stripping() {
        let mut parser =
            AssistantTextStreamParser::new(/*plan_mode*/ true, /*hide_think_tags*/ false);

        let seeded = parser.push_str("Intro\n<proposed");
        let parsed = parser.push_str("_plan>\n- step <oai-mem-citation>doc</oai-mem-citation>\n");
        let tail = parser.push_str("</proposed_plan>\nOutro");
        let finish = parser.finish();

        assert_eq!(seeded.visible_text, "Intro\n");
        assert_eq!(
            seeded.plan_segments,
            vec![ProposedPlanSegment::Normal("Intro\n".to_string())]
        );
        assert_eq!(parsed.visible_text, "");
        assert_eq!(parsed.citations, vec!["doc".to_string()]);
        assert_eq!(
            parsed.plan_segments,
            vec![
                ProposedPlanSegment::ProposedPlanStart,
                ProposedPlanSegment::ProposedPlanDelta("- step \n".to_string()),
            ]
        );
        assert_eq!(tail.visible_text, "Outro");
        assert_eq!(
            tail.plan_segments,
            vec![
                ProposedPlanSegment::ProposedPlanEnd,
                ProposedPlanSegment::Normal("Outro".to_string()),
            ]
        );
        assert!(finish.is_empty());
    }

    #[test]
    fn strips_think_blocks_when_enabled() {
        let mut parser = AssistantTextStreamParser::new(false, true);

        let seeded = parser.push_str("before<th");
        let parsed = parser.push_str("ink>hidden</think>after");
        let tail = parser.finish();

        assert_eq!(seeded.visible_text, "before");
        assert_eq!(parsed.visible_text, "after");
        assert!(tail.is_empty());
    }

    #[test]
    fn preserves_think_blocks_when_disabled() {
        let mut parser = AssistantTextStreamParser::new(false, false);

        let out = parser.push_str("before<think>hidden</think>after");

        assert_eq!(out.visible_text, "before<think>hidden</think>after");
    }
}
