# ICD-9 Triple-Code + CPSBC + Complexity Optimization — Design

**Date:** 2026-06-26
**Status:** Draft (pending implementation)

## Background

BC's physician payment model (PAS/LFP panel system) and CPSBC regulatory reporting both depend on accurate ICD-9 codes attached to patient encounters. Two case-mix systems consume these codes:

1. **Johns Hopkins ACG System (Adjusted Clinical Groups)** — used by BC's LFP/CLFP payment model for patient complexity scoring and panel payments. Maps ICD-9 codes → 32 Aggregated Diagnosis Groups (ADGs) → ~92 ACG categories.
2. **CIHI Population Grouping Methodology (POP Grouper)** — used for population-level health profiling and risk adjustment. Maps ICD-9 codes → 226 health conditions → 164 HPG branches → 239 Health Profile Groups.

Both systems use **all** ICD-9 codes from **all** encounters (not just the primary code), and both reward specificity and completeness. BC MSP claims accept **up to 3 ICD-9 codes per claim** — all three are processed by both systems.

A CPSBC webinar (June 2026, presenter Namrata Jhamb — "Your ICD-9 Codes Matter") codified official do's and don'ts that FerriScribe's prompt should reflect.

Currently, FerriScribe outputs a **single** ICD-9 code per SOAP note. The CPSBC guideline says physicians should submit **up to three** ICD-9 codes per interaction. Each missed code is a missed contribution to the patient's complexity profile.

### How BC's ACG System Works (LFP Panel Payments)

The ACG system is what directly determines per-patient complexity-adjusted panel payments in BC's LFP model:

1. **32 Aggregated Diagnosis Groups (ADGs)**: Every ICD-9 code maps to one of 32 ADGs, which classify diagnoses by expected resource use, duration, severity, and diagnostic certainty.

2. **ACG Categories (~92)**: A patient's unique combination of ADGs determines their ACG category. More distinct ADGs = higher complexity.

3. **Lookback Period**: All ICD-9 codes from all MSP claims over a **12-month period** are considered.

4. **Major ADGs that drive highest complexity**:
   - ADG 3: Time Limited — Major (e.g., acute MI, major trauma)
   - ADG 4: Primary Infection (serious infections requiring treatment)
   - ADG 9: Likely to Recur — Progressive (e.g., metastatic cancer)
   - ADG 11: Chronic Medical — Unstable (e.g., uncontrolled diabetes with complications, COPD exacerbations)
   - ADG 16: Chronic Specialty — Unstable — Orthopedic
   - ADG 22: Injuries — Major
   - ADG 25: Recurrent/Persistent — Unstable — Mental Health (e.g., severe depression, psychosis)
   - ADG 32: Malignancy

5. **Specificity matters**: ICD-9 250.40 (diabetes with renal manifestations) maps to a higher-complexity ADG than 250.00 (diabetes without complication). 4th and 5th digits indicate complications, severity, and specificity — all of which affect ADG assignment.

6. **"Garbage codes" are penalized**: Codes like 780 (general symptoms), 784 (symptoms involving head/neck), 796 (other nonspecific abnormal findings) map to sign/symptom ADGs that are lower complexity than definitive diagnoses.

### How the CIHI POP Grouper Works (Population Profiling)

The POP Grouper operates at the population level and is used for health system planning and risk adjustment:

1. **226 Health Conditions**: Every ICD-9 code maps to one of 226 specific health conditions.
2. **164 HPG Branches Ranked by Complexity**: The 226 conditions roll up into 164 branches, ranked from most to least clinically complex. A patient is assigned to their **highest-ranking** branch.
3. **239 Health Profile Groups (HPGs)**: Each branch is further segmented by presence/absence of **major comorbidities**.
4. **17 HPG Categories**: Major Acute, Major Chronic, Major Cancer, Major Mental Health, Moderate Acute, Moderate Chronic, Minor Acute, Minor Chronic, Palliative, Obstetrics, Other Cancer, Other Mental Health, Non-users, Users without health conditions, Healthy Newborn, Unassigned.
5. **Tagging Rules**: CIHI validates physician billing codes by requiring **minimum instances** of a diagnosis before it's tagged as a confirmed health condition.
6. **Longitudinal Chronic Conditions**: 82 of the 226 health conditions are tracked with index dates over multiple years.
7. **Lookback Period**: 2 years of all clinical data from all sources.

### The Funding Implication

Undercoding — using fewer, less specific, or generic codes — means patients appear healthier than they are in both systems:

- **ACG system**: Fewer distinct ADGs → lower ACG category → lower complexity-adjusted panel payment per patient.
- **POP Grouper**: Fewer health conditions → lower HPG branch rank → under-representation of panel complexity in population risk models.

The Ontario validation study (Li et al., Medical Care, 2019) confirmed: "physicians who enroll patients who are more clinically complex, on average, than is typical will be underpaid in this framework" when coding is incomplete.

## Problem

1. **Only one ICD-9 code per note.** The prompt's `{icd_label}` placeholder says "ICD-9 Code: [specific code reflecting the visit's primary issue]" — singular. MSP claims accept 3; FerriScribe only produces 1. Each missed code is a missed ADG in the ACG system and a missed health condition in the POP Grouper.

2. **No guard against the "780 trap."** The current prompt has no instruction against using the generic 780 (General Symptoms) code as a catch-all. In the ACG system, 780 maps to a low-complexity sign/symptom ADG. In the POP Grouper, it doesn't map to a useful health condition.

3. **No specificity rule.** The prompt doesn't instruct the model to prefer 4- or 5-digit codes over 3-digit codes. Specificity affects ADG assignment in the ACG system and health condition mapping in the POP Grouper.

4. **No guidance on symptom codes during workup.** When a definitive diagnosis hasn't been established yet, CPSBC says to use symptom-based codes. Better than nothing, but definitive codes map to higher-complexity ADGs.

5. **No instruction to code chronic conditions at every relevant visit.** The POP Grouper's tagging rules require minimum instances. The ACG system's 12-month lookback means chronic condition codes need to appear regularly.

6. **No instruction to code comorbidities.** Both systems reward completeness — the POP Grouper uses comorbidity presence for HPG segmentation; the ACG system counts distinct ADGs.

7. **No ordering rule.** The POP Grouper assigns HPG based on the highest-ranking branch. If the model lists codes in arbitrary order, downstream systems may misidentify the primary condition.

## Goals

- FerriScribe should output **up to 3 ICD-9 codes** per SOAP note, ordered by clinical complexity (most complex first), reflecting all distinct conditions actively addressed during the encounter.
- **Optimize for both case-mix systems**: every code should contribute meaningfully to the patient's ACG category (panel payments) and CIHI health condition profile (population risk adjustment).
- Follow CPSBC's official coding guidelines in the prompt.
- Maintain the existing anti-fabrication framework — codes must still be grounded in transcript evidence or clinically inferred (the May 2026 relaxation).
- No new user-facing settings, no schema changes. This is a prompt-only update.

## Non-goals

- External ICD-9 code validation, POP Grouper API calls, ACG lookups, or code tables in the prompt.
- ICD-10 specific changes (ICD-10 remains singular or paired per the existing `icd_version` enum).
- Changes to the structured SOAP output format (section names, bullet style).
- Gaming either system with codes that don't reflect genuine clinical activity.

## Guidelines to Encode

### CPSBC Do's

| Rule | Prompt encoding |
|---|---|
| Submit the most specific ICD-9 codes | OUTPUT FORMAT and SELF-CHECK: "Always use the most specific code available (4- or 5-digit preferred over 3-digit)." |
| Use 4- or 5-digit codes, when possible | Explicit instruction in the ICD code section. |
| Submit up to three ICD-9 codes per interaction | Change `{icd_label}` from singular to a list of up to 3. Change examples to show multiple codes. |
| Use symptom-based codes during diagnostic workup | Add guidance: when a definitive diagnosis hasn't been established, use a symptom code (e.g., 786.50 for chest pain) rather than leaving the field blank. |

### CPSBC Don'ts

| Rule | Prompt encoding |
|---|---|
| Don't submit codes for conditions not addressed at this interaction | Add: "Only include codes for conditions actively addressed during this visit. Do not code historical conditions, resolved problems, or conditions mentioned only in passing." |
| Don't use 780 as a catch-all | Add to FORBIDDEN INFERENCES: "Do not default to 780 (General Symptoms). Prefer specific symptom codes (e.g., 786.50 chest pain, 780.60 fever, 784.0 headache). 780 maps to a low-complexity diagnostic group." |

### Complexity Optimization Rules (ACG + POP Grouper)

| Rule | Rationale | Prompt encoding |
|---|---|---|
| **Order codes by clinical complexity, most complex first** | The POP Grouper assigns HPG based on the highest-ranking branch. The ACG system considers all codes but downstream display may emphasize the first code. | Add to OUTPUT FORMAT: "List codes in order of clinical complexity, most complex first." Add to SELF-CHECK. |
| **Code ALL conditions actively addressed** | Each code contributes a distinct ADG in the ACG system (more distinct ADGs = higher ACG category) and a health condition in the POP Grouper. | Strengthen: "Always aim to identify 3 distinct conditions when the clinical picture supports it." |
| **Code chronic conditions at every visit where they're managed** | ACG's 12-month lookback means regular coding keeps the ADG active. POP Grouper's tagging rules require minimum instances. | Add: "When a chronic condition is managed, assessed, or reviewed at this visit, include its code even if it is not the primary reason for the visit." |
| **Code comorbidities explicitly** | ACG counts distinct ADGs — more distinct condition groups = higher complexity. POP Grouper uses comorbidities for HPG segmentation. | Add: "When multiple conditions are present and addressed, code each distinct condition." |
| **Use specific 4th/5th digit codes** | ICD-9 250.40 (diabetes with renal manifestations) maps to a higher-complexity ADG than 250.00 (diabetes without complication). The 4th/5th digit indicates complications and severity. | Reinforce in the ICD code instruction and SELF-CHECK. |
| **Prefer definitive diagnoses over symptom codes when available** | Definitive diagnoses map to disease-specific ADGs (often higher complexity); symptom codes (780-799) map to lower-complexity sign/symptom ADGs. | Add: "When a definitive diagnosis is established, use the disease-specific code rather than a symptom code." |
| **Code mental health conditions when addressed** | Mental health conditions map to specific ADGs (e.g., ADG 25: Recurrent/Persistent — Unstable — Mental Health) and POP Grouper categories (Major/Other Mental Health). | Implicit in "code all conditions addressed." No special instruction needed. |

## Proposed Prompt Changes

### 1. `icd_code_parts()` — update placeholder text

**Current (ICD-9):**
```
"ICD-9 Code: [specific code reflecting the visit's primary issue. For paperwork-only / wellness / lab-review visits with no diagnosable complaint, use a routine-encounter code such as V70.0.]"
```

**Proposed (ICD-9):**
```
"ICD-9 Codes (up to 3): [List codes in order of clinical complexity, most complex first. Use the most specific code available (4- or 5-digit preferred over 3-digit) — e.g., 250.40 (diabetes with renal manifestations) rather than 250.00 (diabetes without complication). Include all distinct conditions actively addressed, assessed, or managed at this visit — not just the primary complaint. When a chronic condition (diabetes, hypertension, COPD, heart failure, etc.) is managed or reviewed, include its code even if it is not the primary reason for the visit. When a definitive diagnosis is established, use the disease-specific code rather than a symptom code. If no definitive diagnosis exists yet, use a symptom-based code for the presenting complaint (e.g., 786.50 for chest pain). Do not use 780 (General Symptoms) as a catch-all. For paperwork-only / wellness / lab-review visits, use a routine-encounter code such as V70.0. Separate multiple codes with semicolons.]"
```

Equivalent changes for ICD-10 and `both`.

### 2. FORBIDDEN INFERENCES — add two bullets

```
- ICD codes for conditions not addressed at this visit. Do not include codes for historical conditions, resolved problems, chronic conditions mentioned only in passing, or anything the physician did not actively assess or manage during this interaction. Only code conditions with direct clinical activity at this encounter.
- 780 (General Symptoms) as a default or catch-all code. Use 780 only when the presenting complaint genuinely has no more specific symptom code. Prefer specific symptom codes (e.g., 786.50 chest pain NOS, 780.60 fever, 784.0 headache). 780 maps to a low-complexity diagnostic group and contributes little to the patient's clinical profile.
```

### 3. EXAMPLE 1 — update to show triple codes optimized for complexity

**Current:**
```
ICD-9 Code: 847.2 — Sprain of lumbar
```

**Proposed:**
```
ICD-9 Codes:
1. 847.2 — Sprain of lumbar
2. 846.1 — Sprain of lumbosacral ligament
3. 724.2 — Lumbago
```

All three codes are musculoskeletal conditions from the same clinical picture. This visit scenario has limited complexity opportunity — it's a single-issue acute visit. The model should still aim for 3 codes when the clinical picture supports it, and the codes should be specific (4- or 5-digit).

### 4. EXAMPLE 2 — update to show triple codes optimized for complexity

**Current:**
```
ICD-9 Code: 266.2 — Other B-complex deficiencies
```

**Proposed:**
```
ICD-9 Codes:
1. 272.0 — Pure hypercholesterolemia
2. 266.2 — Other B-complex deficiencies
3. V70.0 — Routine general medical examination
```

Code ordering rationale: 272.0 (hypercholesterolemia) is a chronic condition that maps to a chronic ADG — it has longitudinal significance in both the ACG system (12-month lookback) and POP Grouper (2-year lookback with chronic condition tracking). 266.2 (B-complex deficiency) is an acute finding. V70.0 is the encounter type.

### 5. OUTPUT FORMAT section

Update the template line from:
```
{icd_label}
```
to the multi-code format with complexity ordering guidance embedded in the placeholder.

### 6. SELF-CHECK — update item 7

**Current:**
```
7. ICD code check: every ICD code is supported by a transcript-named diagnosis or inferred from findings...
```

**Proposed:**
```
7. ICD code check: up to 3 ICD-9 codes are listed, ordered by clinical complexity (most complex first). Each code uses the most specific 4- or 5-digit version available. Every code represents a distinct condition actively addressed, assessed, or managed at this visit. Chronic conditions managed or reviewed at this visit are included even if not the primary complaint. When a definitive diagnosis is established, the disease-specific code is used rather than a symptom code. No code uses 780 as a catch-all. No code references a condition not addressed at this visit. On paperwork/wellness/lab-only visits, encounter-type codes (e.g., V70.0) are used.
```

## Edge Cases

| Scenario | Expected behaviour | Complexity impact |
|---|---|---|
| Single-issue acute visit (e.g., back pain) | 1 primary code + up to 2 related codes from the same clinical picture. | Limited — acute musculoskeletal conditions map to lower-complexity ADGs. Still code specifically. |
| Multi-issue chronic visit (e.g., diabetes + hypertension + depression) | 3 codes, one per condition addressed, ordered by complexity. | **High** — each distinct condition contributes a different ADG. More distinct ADGs = higher ACG category. Chronic conditions reinforce POP Grouper HPG. |
| Chronic condition follow-up where only the chronic condition is addressed | 1 primary code for the chronic condition + up to 2 comorbidities if discussed. | **High** — regular coding keeps the chronic ADG active in the ACG's 12-month window. Supports POP Grouper tagging rules. |
| Lab review / wellness / no complaint | 1 encounter code (V70.0) + up to 2 codes for conditions found or reviewed. | Moderate — encounter codes are low-complexity, but any abnormal finding coded contributes to the clinical profile. |
| Workup in progress, no diagnosis yet | Symptom-based code (e.g., 786.50) + codes for any incidental conditions addressed. | Moderate — symptom codes map to lower-complexity ADGs, but blank fields contribute nothing. |
| Very brief visit, only 1 condition addressed | 1 code is fine — the prompt says "up to 3", not "exactly 3". | Minimal — but 1 specific code is better than 1 generic code or none. |
| Patient with multiple chronic conditions at an acute visit | Acute condition code first (highest complexity at this visit) + chronic condition codes that were assessed/monitored. | **High** — the acute code maps to a high-complexity ADG; the chronic codes contribute additional distinct ADGs. |
| Diabetes with complications vs. uncomplicated diabetes | Use the specific 4th digit: 250.40 (renal), 250.60 (neurological), 250.80 (other specified) rather than 250.00. | **High** — complications map to higher-complexity ADGs than the base condition. |

## The Complexity Optimization Principle

**Code honestly but code completely.** The goal is not to game the system, but to ensure that the ICD-9 codes submitted at each encounter reflect the **true clinical complexity** of the patient. Every condition the physician addresses, manages, monitors, or reviews at a visit deserves a code — because each code contributes to:

- A distinct ADG in the ACG system (driving per-patient panel payment)
- A health condition in the POP Grouper (driving population risk adjustment)
- Longitudinal chronic condition tracking (ensuring chronic diseases are recognized as ongoing)

The difference between coding 1 condition at a multi-issue chronic care visit vs coding all 3 conditions addressed is the difference between a patient appearing as a simple single-chronic-condition case and a multi-morbid complex patient. That difference flows through to panel payments, resource allocation, and population health planning.

## Testing

- Update the two existing examples in the prompt to show triple codes with complexity ordering.
- Add a unit test verifying the `{icd_label}` placeholder resolves to multi-code format.
- Manual QA: run the prompt against 5-10 real transcripts and verify codes are specific, relevant, correctly ordered, and include chronic conditions.
- Check that the `(suggested)` convention still applies to inferred codes per the May 2026 design.
- Verify that chronic conditions are coded at follow-up visits, not just at the initial diagnosis visit.

## Implementation

Single-file change: `crates/processing/src/soap_generator/prompt_template.rs`
- Edit `icd_code_parts()` for all three variants (ICD-9, ICD-10, both)
- Edit the prompt body: FORBIDDEN INFERENCES, both examples, OUTPUT FORMAT, SELF-CHECK
- Update existing tests and add new test for triple-code format

## References

- CIHI Population Grouping Methodology v1.4: Overview and Outputs (2023)
- CIHI POP Grouper Information Sheet
- Li Y, Weir S, Steffler M, et al. "Using Diagnoses to Estimate Health Care Cost Risk in Canada." Medical Care. 2019;57(11):875-881.
- HSPN Population Segmentation Presentation (Sept 2021)
- Alberta Medical Association: PCPCM Complexity-Adjusted Panel Payments
- Johns Hopkins ACG System — Aggregated Diagnosis Groups documentation
- BC MSP Claims — up to 3 ICD-9 codes per claim
- CPSBC webinar: "Your ICD-9 Codes Matter" (June 2026, presenter Namrata Jhamb)
