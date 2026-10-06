/// `_structure_plan_contract`
pub fn structure_plan() -> &'static str {
    r#"Return strict JSON only:
{
  "structure_plan": {
    "visual_family": "pipeline|architecture|mechanism|comparison|multi-panel|other",
    "canvas_ratio_hint": "16:9|4:3|1:1|inherit",
    "primary_flow": "left-to-right|top-to-bottom|...",
    "lanes_or_stages": [
      {"name": "stage/lane abstract name", "role": "what this band does", "region": "top|middle|bottom|left|right", "slot_count": 3}
    ],
    "module_slots": [
      {"id": "m1", "role": "abstract role e.g. encoder-like", "stage": "stage name", "region": "left-middle"}
    ],
    "connection_slots": [
      {"from": "m1", "to": "m2", "style": "solid|dashed", "meaning_role": "data|control|feedback"}
    ],
    "grouping": "how panels/cards/nested boxes are organized",
    "information_density": "high|medium",
    "palette_and_rhythm": "short note on box style, color mood, spacing rhythm",
    "layout_skeleton": "80+ chars free-text blueprint for later content filling (no user research claims)"
  }
}
Use at least 2 lanes_or_stages, 6 module_slots, and 5 connection_slots. Roles must be abstract, not copied caption text."#
}

/// `_paper_contract`
pub fn paper() -> &'static str {
    r#"Return strict JSON only. Choose diagram/workflow/comparison/mechanism OR plot/chart.
CRITICAL: Do not over-summarize the user section. Keep leaf operations (OCR, BM25, dual-tower, cross-encoder, Top-K, etc.) as modules or explicit sub-labels.
Fill structure_plan layout with user content. Prefer the user brief language for on-figure labels (Chinese brief → Chinese labels).
Contract:
{
  "figure": {
    "title": "...",
    "visual_type": "diagram|workflow|comparison|mechanism|plot|chart",
    "aspect_ratio": "inherit|16:9|4:3|1:1|3:2|2:3|9:16",
    "template_usage": "strict|balanced|loose",
    "layout_constraints": ["canvas, composition, hierarchy, spacing, structure_plan alignment"],
    "semantic_constraints": ["faithfulness rules grounded in the user section; no dropped subprocesses"],
    "visual_constraints": ["publication quality, palette, typography, contrast, readable dense layout"],
    "forbidden_errors": ["hallucination", "reversed flow", "scope violation", "text overload", "over-simplification", "direct template copying"],
    "quality_rubric": {
      "faithfulness": "retain user-listed operations",
      "conciseness": "short labels, not fewer steps",
      "readability": "...",
      "aesthetics": "..."
    },
    "content_inventory": ["12-25 grounded operations/components extracted from user section"],
    "visible_text": ["short labels covering inventory"],
    "diagram_spec": {
      "modules": ["≥8 grounded short names covering inventory leaf steps"],
      "entities": ["artifacts e.g. 文档块/证据包/索引"],
      "connections": [{"source": "...", "target": "...", "meaning": "..."}],
      "flow_direction": "left-to-right primary flow (or top-to-bottom)",
      "grouping_hierarchy": "outer stages with nested leaf modules",
      "arrow_routing": "solid data + dashed control/feedback",
      "label_strategy": "keyword labels; keep leaf count"
    },
    "plot_spec": {
      "chart_type": "...",
      "data_fields": ["..."],
      "axes": {"x": "label and unit", "y": "label and unit"},
      "units": "... or none",
      "series_or_categories": ["..."],
      "legend": "...",
      "statistical_annotations": "none or supported annotations only",
      "data_integrity_rules": "No value distortion, misleading scales, label fabrication, wrong chart type, or unsupported statistics."
    },
    "information_units": [{"unit": "one information unit from the material", "carrier": "table|chart|diagram|text|icon|object", "reason": "why this single carrier"}],
    "redundancy_check": {"removed": ["duplicate expressions merged or dropped"], "statement": "confirm no graphic-graphic, text-text, or graphic-text semantic duplication"},
    "hierarchy_plan": {"levels": [{"level": "title|section|body|caption", "font_size": "...", "weight": "...", "color": "..."}], "alignment": "edge/baseline/grid alignment rules", "focus_region": "where the core content sits in the main body area"},
    "implement_prompt": "Long bilingual-capable drawing brief WITH required sections: (1) canvas/layout (2) stage list (3) MODULE DETAIL / 模块细节 per stage listing every leaf module and edges (4) arrows (5) style (6) faithfulness/forbidden/template boundary. Must restate key user terms (OCR/BM25/双塔/重排/Top-K/… when present). Must restate the expression constraints: one carrier per information unit, uniform font size for same-level text, aligned edges/baselines, core content in the main region."
  },
  "quality_checklist": ["detail-preserving", "multi-stage", "faithful", "readable"]
}
Omit `diagram_spec` only for plot/chart. Omit `plot_spec` only for diagram/workflow/comparison/mechanism."#
}

/// `_advisor_contract`
pub fn advisor() -> &'static str {
    r#"Return strict JSON only:
{
  "advice": {
    "reusable_layout": ["layout/expression patterns from the candidates that fit the new brief; empty if none"],
    "term_mapping": [{"from": "term in the new brief", "to": "preferred label", "reason": "why"}],
    "failure_modes_to_avoid": ["concrete failure modes seen in or likely from the candidates"],
    "confidence": "high|medium|low",
    "notes": "one short sentence on how similar the best candidate is"
  }
}
Keep every list short (at most 6 items). Never copy candidate research content into the advice."#
}

pub fn template_analysis() -> &'static str {
    r##"Return strict JSON only with professional PPT master analysis:
{
  "template_analysis": {
    "master_style_summary": "...",
    "master_style_spec": {
      "canvas": {"aspect_ratio": "16:9", "orientation": "landscape", "background": "..."},
      "title_region": {"position": "normalized coordinates", "alignment": "...", "hierarchy": "...", "reserved_whitespace": "..."},
      "safe_margins": {"top": "...", "right": "...", "bottom": "...", "left": "...", "body_area": "..."},
      "header_footer": {"page_number": "...", "logo": "...", "corner_marks": "...", "reserved_regions": ["..."]},
      "divider_lines": [{"position": "...", "stroke": "...", "color": "..."}],
      "palette": {"background": "#...", "primary": "#...", "secondary": "#...", "accent": "#...", "neutral": "#...", "forbidden_drift": "..."},
      "typography": {"title": "...", "subtitle": "...", "body": "...", "caption": "...", "alignment": "..."},
      "module_style": {"border": "...", "radius": "...", "fill": "...", "shadow": "...", "spacing": "..."},
      "decorative_elements": ["..."],
      "immutable_elements": ["title region", "page number/logo/corner marks", "divider lines", "background", "palette", "typography", "module/card style"],
      "page_layout_rules": "How A/B/C body skeletons may vary inside safe body area only.",
      "forbidden_deviations": ["moving master elements", "palette drift", "new logo/page number", "changing title region", "overflowing safe margins"]
    },
    "global_constraints": ["..."],
    "immutable_elements": ["..."],
    "page_layout_rules": "..."
  }
}"##
}

/// `_ppt_outline_contract`
pub fn ppt_outline(page_count: usize) -> String {
    format!(
        r#"Return strict JSON only. Deck outline mode: return exactly {page_count} lightweight page briefs and a shared prompt. Do not write page-level implement_prompt here.
{{
  "deck_outline": {{
    "deck_title": "Simplified Chinese deck title",
    "deck_goal": "What the deck must communicate to the audience.",
    "narrative_arc": "How page 1..{page_count} progress logically without repetition.",
    "shared_prompt": {{
      "audience": "target audience",
      "tone": "academic presentation tone",
      "global_style_constraints": "Use the extracted template master as immutable; body layouts vary only inside safe area.",
      "terminology": ["consistent key terms"],
      "visual_language": "Use proportionate icons, logo-like/product/object visuals when useful and source-grounded.",
      "emphasis_language": "Use bold or template accent red for 2-5 short phrases per page only."
    }},
    "page_briefs": [
      {{
        "page": 1,
        "title": "Simplified Chinese slide title",
        "role": "cover|problem|method|experiment|result|summary|transition|technical body",
        "main_message": "One-sentence page claim.",
        "content_points": ["3-5 concise content points grounded in material"],
        "suggested_template": "Template A|Template B|Template C-1|Template C-2|Template C-3|Template C-4|Template C-5|Template C-6|Template C-7",
        "visual_direction": "Suggested visual/icon/object/chart direction for this page.",
        "transition_from_previous": "How this page connects from previous page, or none for page 1.",
        "transition_to_next": "How this page leads to next page, or closure for last page."
      }}
    ]
  }}
}}"#
    )
}

/// `_ppt_single_page_contract`
pub fn ppt_single_page(page_number: usize) -> String {
    format!(
        r#"Return strict JSON only. Single-page worker mode: return exactly one page object for page {page_number}. Do not output other pages. Do not put API output parameters such as size, quality, output_format, response_format, aspect_ratio, image_size, thinking_level, or mime_type into prompt text.
{{
  "page": {{
    "page": {page_number},
    "selected_template": "Template A|Template B|Template C-1|Template C-2|Template C-3|Template C-4|Template C-5|Template C-6|Template C-7",
    "title": "Simplified Chinese slide title from the current page brief",
    "slide_type": "...",
    "body_layout_plan": "Describe only this page's variable body skeleton inside the safe body area, including text/visual balance.",
    "master_style_binding": {{
      "title_region": "same extracted title region and title hierarchy",
      "safe_margins": "same body safe area and no-overflow margins",
      "header_footer": "same page number/logo/corner marks/header/footer behavior",
      "divider_lines": "same divider/separator geometry and stroke",
      "palette": "same background/primary/accent/neutral colors",
      "typography": "same font hierarchy, weights, sizes, and alignment",
      "module_style": "same card/frame/border/radius/fill/shadow/spacing rhythm",
      "background": "same background treatment"
    }},
    "visual_element_plan": {{
      "usage_decision": "Default to depicting real subjects. Name what will actually be drawn, or explicitly justify why this page is too abstract for any depiction.",
      "elements": [
        {{
          "type": "object/device render|specimen or material illustration|schematic cutaway|scene illustration|product/tool mark|logo-like symbol|semantic icon|none",
          "subject": "what the visual represents",
          "appearance": "Concrete look: overall shape and proportion, dominant materials and colors, defining structural features, typical orientation. The implement model has no other source for this.",
          "source_reference": "source URL/title from Visual Asset Search Context, or 'domain knowledge, generic form' when no source is available",
          "placement": "where it sits inside the safe body area",
          "style": "must be recolored into the template palette and match template line weight, card/border style, and academic restraint",
          "size_ratio": "small|medium|large with approximate body-area percentage"
        }}
      ],
      "text_visual_balance": "Explain how visual elements and text remain proportionate and readable."
    }},
    "emphasis_plan": {{
      "keywords": [{{"text": "short Chinese key phrase", "style": "bold|template-primary-red|template-accent-red", "reason": "why this phrase is emphasized"}}],
      "style_rules": "Highlight only 2-5 short key phrases per page; never mark whole sentences or drift from template palette."
    }},
    "visible_text": ["Simplified Chinese visible text only, short strings"],
    "information_units": [{{"unit": "one information unit from the material", "carrier": "table|chart|diagram|text|icon|object", "reason": "why this single carrier"}}],
    "redundancy_check": {{"removed": ["duplicate expressions merged or dropped"], "statement": "confirm no graphic-graphic, text-text, or graphic-text semantic duplication"}},
    "hierarchy_plan": {{"levels": [{{"level": "title|section|body|caption", "font_size": "...", "weight": "...", "color": "..."}}], "alignment": "edge/baseline/grid alignment rules", "focus_region": "where the core content sits in the main body area"}},
    "implement_prompt": "Page-specific body instructions only. Start with: Create one 16:9 academic PowerPoint-style slide. Describe this page's variable body content, layout skeleton, visual/icon/object/product elements, diagrams/charts, keyword emphasis, and visible Chinese text. Restate the expression constraints: one carrier per information unit, uniform font size for same-level text, aligned edges/baselines, core content in the main region. Do not repeat API output settings. Do not rely on the uploaded image or network images being available to the implement model."
  }}
}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contracts_expose_strengthened_fields() {
        for token in [
            "diagram_spec",
            "plot_spec",
            "quality_rubric",
            "data_integrity_rules",
            "information_units",
            "redundancy_check",
            "hierarchy_plan",
        ] {
            assert!(paper().contains(token), "paper contract missing {token}");
        }
        for token in [
            "master_style_spec",
            "immutable_elements",
            "forbidden_deviations",
        ] {
            assert!(
                template_analysis().contains(token),
                "template contract missing {token}"
            );
        }
        let single = ppt_single_page(1);
        for token in [
            "master_style_binding",
            "safe_margins",
            "module_style",
            "visual_element_plan",
            "emphasis_plan",
            "information_units",
            "redundancy_check",
            "hierarchy_plan",
        ] {
            assert!(
                single.contains(token),
                "single page contract missing {token}"
            );
        }
        let outline = ppt_outline(2);
        for token in ["deck_outline", "shared_prompt", "page_briefs"] {
            assert!(outline.contains(token), "outline missing {token}");
        }
        for token in ["information_units", "redundancy_check", "hierarchy_plan"] {
            assert!(!outline.contains(token), "outline must not require {token}");
            assert!(
                !template_analysis().contains(token),
                "template contract must not require {token}"
            );
            assert!(
                !structure_plan().contains(token),
                "structure plan must not require {token}"
            );
        }
        assert!(outline.contains("Do not write page-level implement_prompt"));
        assert!(!outline.contains("\"implement_prompt\":"));
        assert!(!single.contains("size="));
        assert!(single.contains("Do not put API output parameters"));
    }

    #[test]
    fn advisor_contract_names_the_three_suggestion_kinds() {
        for token in ["reusable_layout", "term_mapping", "failure_modes_to_avoid"] {
            assert!(
                advisor().contains(token),
                "advisor contract missing {token}"
            );
        }
    }

    #[test]
    fn single_page_contract_requires_appearance() {
        assert!(ppt_single_page(3).contains("\"appearance\""));
    }

    #[test]
    fn structure_plan_states_minimums() {
        let contract = structure_plan();
        assert!(
            contract.contains("at least 2 lanes_or_stages, 6 module_slots, and 5 connection_slots")
        );
    }
}
