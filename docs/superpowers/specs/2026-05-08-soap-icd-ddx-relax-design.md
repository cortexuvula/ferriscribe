# SOAP ICD + Differential Diagnosis Relaxation — Design

**Date:** 2026-05-08
**Status:** Approved (ready for implementation plan)

## Problem

The SOAP system prompt (`crates/processing/src/soap_generator/prompt_template.rs`) is hardened against fabrication: every line of output must trace back to a transcript quote, otherwise the model writes "Not discussed" / "Not recorded" / "Not applicable." The hardening is correct for most sections — demographics, vitals, medications, follow-up intervals, red-flag warnings — but it has two unintended side effects on real-world clinical workflow:

1. **ICD codes only appear when a diagnosis was explicitly stated.** Lab-review, wellness, and brief follow-up visits routinely produce notes whose ICD line reads "Not applicable - no diagnosis clearly discussed." The clinician is left to manually add the code. The model has the clinical knowledge to suggest a plausible code from the visit's findings; the prompt forbids it.

2. **Differential diagnoses only appear when explicitly discussed.** In practice physicians rarely *verbalise* a differential — they form one mentally and then act on the most likely. The current rule produces "No differential diagnoses were discussed during the visit" on almost every note, removing what would otherwise be the most useful decision-support output the scribe can offer.

The user wants both slots loosened to permit clinical inference, while keeping every other anti-fabrication guard intact.

## Goals

- Always output a specific ICD-9 (or ICD-10, or both, per `icd_version` setting) code for every SOAP note. When the diagnosis was named by the physician in the transcript, the code is rendered plain. When the model inferred it from findings, it is marked `(suggested)`.
- Always output **at least three** differential diagnoses. Each item is plain if physician-stated, `(suggested)` if model-inferred.
- The `(suggested)` convention is explicit, consistent, and surfaced in the example output, the OUTPUT FORMAT block, and the SELF-CHECK block.
- Pure paperwork / wellness / lab-only visits use encounter-type codes (e.g. ICD-9 V70.0 for a routine adult exam) and a plausible-by-pattern DDx — still all marked `(suggested)`.
- All other FORBIDDEN INFERENCES categories (demographics, dosages, follow-up intervals, red-flag warnings, etc.) remain strict.
- No new user-facing settings, no schema changes, no new toggles. The behavior is the new default; users who prefer the strict version can supply a custom prompt.

## Non-goals

- A separate "decision support" mode behind a settings flag.
- Changing how `icd_version` is selected or stored — the existing `IcdVersion::{Icd9, Icd10, Both}` enum and settings field stay as is.
- Any change to the structured SOAP output format (section names, bullet style, output-line conventions).
- Loosening the anti-fabrication rules anywhere outside ICD and Differential Diagnosis.
- Adding ICD-code lookups, validation, or external API calls. The model's training-time knowledge is the source.
- A schema for "(suggested)" — it is a string convention in plain text, not a structured field. Downstream tools that want to detect inferred items can grep for the literal substring.

## Decisions

| # | Decision |
|---|---|
| Q1 | Rollout: new default for everyone. No toggle, no per-template gating. Custom prompts still override. |
| Q2 | Marking convention: every inferred item (ICD or DDx) is suffixed with literal `(suggested)`. Items explicitly named by the physician are rendered plain. |
| Q3 | Edge case: pure paperwork / wellness / lab-only visits still produce both an ICD code (encounter-type code, marked `(suggested)`) and three DDx (all `(suggested)`, anchored on whatever findings or encounter context exists). |
| Q4 | The other 11 FORBIDDEN INFERENCES categories stay strict. ICD and Differential Diagnosis are the *only* sections where clinical inference is permitted, and the prompt says so explicitly. |
| Q5 | The `(suggested)` marker is plain text — no structured field, no JSON, no parser. Downstream consumers can grep. |
| Q6 | At-least-three is the floor; the prompt does not cap the number. The model picks how many beyond three are clinically useful. |

## Architecture

This is a single-file change. The `default_soap_prompt()` constant body and the `icd_code_parts()` helper in `crates/processing/src/soap_generator/prompt_template.rs` are edited; the test module in the same file is updated; nothing else moves.

### Edits inside the prompt

#### 1. `icd_code_parts()` — placeholder text

Current `(icd_instruction, icd_label)` for ICD-9:
```
"ICD-9 code"
"ICD-9 Code: [code if a primary diagnosis was clearly discussed; otherwise \"Not applicable - no diagnosis clearly discussed\"]"
```

New:
```
"ICD-9 code"
"ICD-9 Code: [specific code reflecting the visit's primary issue; append (suggested) if the physician did not explicitly name the diagnosis. For paperwork-only / wellness / lab-review visits with no diagnosable complaint, use a routine-encounter code (e.g. V70.0 for ICD-9, Z00.00 for ICD-10) and mark it (suggested).]"
```

Equivalent rewrites for ICD-10 (`Z00.00`) and `both`. Encounter-code suggestions are illustrative, not exhaustive — the prompt does not enumerate codes.

#### 2. FORBIDDEN INFERENCES list

Remove the bullet:
```
- ICD codes when no diagnosis was clearly discussed. If no clear primary diagnosis is stateable from the transcript, write "Not applicable - no diagnosis clearly discussed" instead of guessing a code.
```

Replace it with a positive bullet that frames the new exception explicitly:
```
- ICD codes and differential diagnoses are the ONLY two sections where clinical inference is permitted. Every inferred item must be marked with the literal text "(suggested)". Items the physician explicitly named in the transcript are rendered plain (no marker). All other categories above remain strict — do not extend this exception to demographics, dosages, follow-up intervals, red-flag warnings, or any other section.
```

#### 3. EXAMPLE 1 (back-strain visit) — negative-list cleanup

Current negative-list line to remove:
```
- "Rule out disc herniation" or any differential diagnosis (none discussed)
```

In the EXAMPLE 1 body, leave the Subjective/Objective/Plan/Follow-up output unchanged (it is a tight visit with no diagnostic ambiguity), but ADD a Differential Diagnosis block with three (suggested) items consistent with the lifting-injury picture:

```
Differential Diagnosis:
- Lumbar muscle strain (suggested)
- Lumbar facet sprain (suggested)
- Lumbar disc herniation (suggested)
```

…and an ICD line above Subjective:

```
ICD-9 Code: 847.2 — Sprain of lumbar (suggested)
```

(Equivalent ICD-10: M54.5. The example will use whichever the prompt's resolved `icd_label` produces; for clarity in the spec, ICD-9 is shown.)

The "what this deliberately does NOT contain" list keeps every other entry — vitals, exam findings, general appearance, red-flags, allergy/medication invention. The DDx item is removed; everything else stays as a fabrication.

#### 4. EXAMPLE 2 (lab review)

Replace:
```
ICD-10 Code: Not applicable - no diagnosis clearly discussed
…
Differential Diagnosis:
- No differential diagnoses were discussed during the visit
```

With (using ICD-9 since the prompt default is ICD-9):
```
ICD-9 Code: 266.2 — Other B-complex deficiencies (suggested)

…

Differential Diagnosis:
- Vitamin B12 deficiency (suggested)
- Lipoprotein(a) elevation contributing to atherosclerotic cardiovascular risk (suggested)
- Mixed hyperlipidemia (suggested)
```

Update the EXAMPLE 2 negative-list:
- Remove `An ICD code (no clear diagnosis was made — write "Not applicable")`
- Keep every other negative-list entry (demographics, PMH, medications, family history, social history, B12 dose, modality, referral, follow-up interval, red-flags).

#### 5. OUTPUT FORMAT Differential Diagnosis block

Replace:
```
Differential Diagnosis:
- [Only diagnoses explicitly discussed during the visit. If none discussed: "- No differential diagnoses were discussed during the visit"]
```

With:
```
Differential Diagnosis:
- [List at least three diagnoses, ranked by clinical likelihood given the chief complaint and findings. Each item: plain if the physician explicitly named it; suffixed with "(suggested)" if you inferred it from findings. On a paperwork-only / wellness / lab-only visit with no chief complaint, list three plausible items consistent with the encounter type or the labs reviewed, all marked (suggested).]
```

#### 6. OUTPUT FORMAT Assessment block (small touch)

The current Assessment template includes the inline ICD instruction:
```
Include {icd_instruction} inline if a primary diagnosis was clearly discussed; otherwise omit the code.
```

Update to:
```
Include {icd_instruction} inline; mark with "(suggested)" if you inferred the diagnosis. The same code goes on the {icd_label} line above; the inline mention in Assessment is permitted but not required.
```

#### 7. SELF-CHECK block — items 7 and 10

Current item 7:
```
7. ICD code check: only include a code if a clear primary diagnosis was discussed. If not, write "Not applicable - no diagnosis clearly discussed."
```

Replace with:
```
7. ICD code check: every ICD code is either supported by a transcript-named diagnosis (no marker) or inferred from findings (marked "(suggested)"). Never output a bare code without one of these. Never output "Not applicable" — use an encounter-type code (e.g. V70.0 / Z00.00) marked (suggested) on paperwork-only visits.
```

Add a new item 10 at the end of the SELF-CHECK enumeration:
```
10. Differential Diagnosis count + marker check: the Differential Diagnosis section contains at least three items. Each item is either physician-stated (plain) or marked (suggested). If fewer than three are stateable from the transcript, fill the remaining slots with (suggested) items consistent with the chief complaint or findings.
```

(Renumber if needed; the existing 9 numbered items stay in order.)

### What does NOT change

- All other FORBIDDEN INFERENCES bullets (demographics, comorbidities, medications & dosages, family history, social history, modality, general appearance, provider names, follow-up intervals, red-flag warnings).
- SELF-CHECK items 1-6, 8-9.
- Section structure, formatting rules, dash-prefix convention, "Plain text only, no markdown", section ordering.
- Cache-busting structural rules (EXAMPLE before OUTPUT FORMAT, SELF-CHECK at the end).
- Rule 2 ("transcript is the sole source of truth") — but the new exception bullet in FORBIDDEN INFERENCES makes the ICD/DDx carve-out explicit.
- `SoapPromptConfig` struct, `IcdVersion` enum, settings persistence, settings UI.

### Edits inside the test module

Existing tests in the file's `#[cfg(test)] mod tests` block:

| Test | Update |
|------|--------|
| `default_soap_prompt_has_structure_markers` | No change — section names unchanged. |
| `default_soap_prompt_includes_few_shot_example` | No change. |
| `default_soap_prompt_includes_self_check_block` | No change — block still present. |
| `self_check_block_is_at_end_for_recency` | No change. |
| `example_appears_before_output_format` | No change. |
| `default_soap_prompt_resolves_icd9` | Drop `"Not applicable - no diagnosis clearly discussed"` assertion; add `"(suggested)"` assertion. Keep the placeholder-resolved assertion. |
| `default_soap_prompt_resolves_icd10` | Same. |
| `default_soap_prompt_resolves_both_icd` | Same. |
| `default_soap_prompt_includes_forbidden_inferences_block` | Drop the `"ICD codes when no diagnosis"` assertion; keep every other category assertion. |
| `default_soap_prompt_includes_lab_review_example` | Drop `"Not applicable - no diagnosis clearly discussed"` assertion. Add: lab-review block contains an ICD code line with `(suggested)`, AND the Differential Diagnosis block has at least 3 items (counted by lines beginning with `- ` between the "Differential Diagnosis:" header and the next blank line). |
| `self_check_lists_category_checks` | Add `"Differential Diagnosis count"` to the asserted-substring list. Keep the existing `"ICD code check"` (its body changed but the label stays). |
| `default_soap_prompt_includes_template_guidance` | No change. |
| `current_medications_format_allows_supplementary_background` | No change. |
| `historical_subjective_fields_allow_supplementary_background` | No change. |
| `medication_self_check_allows_supplementary_background` | No change. |
| `default_soap_prompt_treats_patient_record_as_authoritative` | No change. |
| `template_specific_instructions` | No change. |
| `custom_soap_prompt_overrides_default` | No change. |
| `empty_custom_prompt_falls_back_to_default` | No change. |

New tests to add:

- `default_soap_prompt_requires_at_least_three_differentials` — asserts the OUTPUT FORMAT Differential Diagnosis block contains the substring `"at least three"`.
- `default_soap_prompt_explains_suggested_marker_convention` — asserts the FORBIDDEN INFERENCES block contains the substring `"(suggested)"` AND the substring `"only two sections where clinical inference is permitted"` (or equivalent locked phrasing).
- `default_soap_prompt_drops_old_icd_blocking_rule` — asserts the prompt does NOT contain the old `"Not applicable - no diagnosis clearly discussed"` exhortation in the FORBIDDEN INFERENCES list (it can still appear in the SELF-CHECK item 7 as a forbidden output, with the negation explicit).
- `default_soap_prompt_lab_review_example_has_three_differentials` — given the lab-review example block, count items in its Differential Diagnosis section; assert >= 3 and assert each ends with `(suggested)`.
- `default_soap_prompt_self_check_keeps_other_strict_categories` — sanity test that demographics, medication, referral, follow-up, red-flag, and visit-modality checks are all still in the SELF-CHECK block (regression guard against accidentally weakening unrelated categories).

## Error handling

- The model occasionally outputs a code that doesn't exist or is mistyped. The prompt does not validate codes; downstream rendering displays whatever the model produces, marker and all. This is consistent with how the rest of the prompt works (the model's medication-name output isn't validated against a drug DB either).
- The model occasionally returns fewer than three DDx items. The SELF-CHECK rewrite makes this a category-check failure that the model must fix before output; in practice this is a soft guard. Acceptable. We do not parse and reject the model's output.
- The model occasionally puts `(suggested)` on every item including physician-stated ones. Acceptable failure mode — the marker over-fires rather than under-fires, which is the safer direction.
- The model occasionally drops the `(suggested)` marker on items it inferred. This is the dangerous failure direction. The EXAMPLE 2 demonstration is the strongest defense; the SELF-CHECK item 10 is the secondary defense. We will manually QA a handful of real transcripts after merging.

## Out of scope

- Code validation, ICD lookup, ICD-10-CM vs ICD-10-WHO disambiguation.
- Detecting and surfacing low-confidence inferences in the UI.
- A "show only suggested items" filter view in the rendered SOAP note.
- Telehealth template tweaks (the existing `template_guidance_text` for `Telehealth` already notes "remote-exam limitations" — no change to that string).
- Differential ranking confidence scores.
- Any change to `crates/agents/src/agents/compliance.rs` or other downstream consumers of the SOAP output.
