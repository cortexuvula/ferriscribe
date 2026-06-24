use medical_core::types::rag::ExpandedQuery;
use std::collections::HashMap;

/// Expands medical queries with abbreviations and synonyms to improve retrieval.
pub struct QueryExpander {
    abbreviations: HashMap<String, Vec<String>>,
    synonyms: HashMap<String, Vec<String>>,
}

impl QueryExpander {
    /// Create a new `QueryExpander` pre-loaded with the default medical
    /// abbreviation and synonym dictionaries.
    pub fn new() -> Self {
        Self {
            abbreviations: default_abbreviations(),
            synonyms: default_synonyms(),
        }
    }

    /// Expand a query by substituting abbreviations and adding synonyms.
    ///
    /// The returned [`ExpandedQuery`] contains the original query unchanged, a
    /// deduplicated list of expansion terms not already present in the original,
    /// and a `full_query` combining both.
    pub fn expand(&self, query: &str) -> ExpandedQuery {
        let original = query.to_string();
        let lower = query.to_lowercase();
        let tokens: Vec<&str> = lower.split_whitespace().collect();

        let mut extra: Vec<String> = Vec::new();

        // Single-token abbreviation expansion
        for token in &tokens {
            if let Some(expansions) = self.abbreviations.get(*token) {
                for exp in expansions {
                    extra.push(exp.clone());
                }
            }
        }

        // Multi-word phrase synonym lookup (windows of 2, 3, 4 tokens)
        for window_size in 2..=4usize {
            for window in tokens.windows(window_size) {
                let phrase = window.join(" ");
                if let Some(syns) = self.synonyms.get(&phrase) {
                    for s in syns {
                        extra.push(s.clone());
                    }
                }
            }
        }

        // Single-token synonym lookup as well
        for token in &tokens {
            if let Some(syns) = self.synonyms.get(*token) {
                for s in syns {
                    extra.push(s.clone());
                }
            }
        }

        // Deduplicate and remove terms already present in the original query
        let original_lower = lower.clone();
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut expanded_terms: Vec<String> = Vec::new();

        for term in extra {
            let term_lower = term.to_lowercase();
            // Skip if already in original query or already added
            if original_lower.contains(&term_lower) {
                continue;
            }
            if seen.contains_key(&term_lower) {
                continue;
            }
            seen.insert(term_lower, ());
            expanded_terms.push(term);
        }

        let full_query = if expanded_terms.is_empty() {
            original.clone()
        } else {
            format!("{} {}", original, expanded_terms.join(" "))
        };

        ExpandedQuery {
            original,
            expanded_terms,
            full_query,
        }
    }
}

impl Default for QueryExpander {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns a mapping of medical abbreviations to their expansions.
fn default_abbreviations() -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();

    // Cardiovascular
    m.insert("htn".into(), vec!["hypertension".into()]);
    m.insert(
        "chf".into(),
        vec!["congestive heart failure".into(), "heart failure".into()],
    );
    m.insert(
        "mi".into(),
        vec!["myocardial infarction".into(), "heart attack".into()],
    );
    m.insert("afib".into(), vec!["atrial fibrillation".into()]);
    m.insert("cad".into(), vec!["coronary artery disease".into()]);
    m.insert("dvt".into(), vec!["deep vein thrombosis".into()]);
    m.insert("pe".into(), vec!["pulmonary embolism".into()]);

    // Respiratory
    m.insert(
        "copd".into(),
        vec!["chronic obstructive pulmonary disease".into()],
    );
    m.insert(
        "sob".into(),
        vec!["shortness of breath".into(), "dyspnea".into()],
    );
    m.insert("uri".into(), vec!["upper respiratory infection".into()]);

    // Endocrine
    m.insert(
        "dm".into(),
        vec!["diabetes mellitus".into(), "diabetes".into()],
    );
    m.insert(
        "t2dm".into(),
        vec!["type 2 diabetes mellitus".into(), "type 2 diabetes".into()],
    );
    m.insert(
        "t1dm".into(),
        vec!["type 1 diabetes mellitus".into(), "type 1 diabetes".into()],
    );
    m.insert("tsh".into(), vec!["thyroid stimulating hormone".into()]);

    // Neurological
    m.insert(
        "cva".into(),
        vec!["cerebrovascular accident".into(), "stroke".into()],
    );
    m.insert("tia".into(), vec!["transient ischemic attack".into()]);
    m.insert("ms".into(), vec!["multiple sclerosis".into()]);

    // Gastrointestinal
    m.insert(
        "gerd".into(),
        vec![
            "gastroesophageal reflux disease".into(),
            "acid reflux".into(),
        ],
    );
    m.insert("ibs".into(), vec!["irritable bowel syndrome".into()]);

    // Renal
    m.insert("ckd".into(), vec!["chronic kidney disease".into()]);
    m.insert("uti".into(), vec!["urinary tract infection".into()]);
    m.insert("aki".into(), vec!["acute kidney injury".into()]);

    // Labs / vitals
    m.insert("bmi".into(), vec!["body mass index".into()]);
    m.insert("bp".into(), vec!["blood pressure".into()]);
    m.insert("hr".into(), vec!["heart rate".into()]);
    m.insert("rr".into(), vec!["respiratory rate".into()]);
    m.insert("wbc".into(), vec!["white blood cell count".into()]);
    m.insert("rbc".into(), vec!["red blood cell count".into()]);
    m.insert("hgb".into(), vec!["hemoglobin".into()]);
    m.insert(
        "plt".into(),
        vec!["platelet count".into(), "platelets".into()],
    );
    m.insert("bun".into(), vec!["blood urea nitrogen".into()]);
    m.insert("cr".into(), vec!["creatinine".into()]);
    m.insert("inr".into(), vec!["international normalized ratio".into()]);
    m.insert("esr".into(), vec!["erythrocyte sedimentation rate".into()]);
    m.insert("crp".into(), vec!["c-reactive protein".into()]);
    m.insert(
        "hba1c".into(),
        vec!["hemoglobin a1c".into(), "glycated hemoglobin".into()],
    );
    m.insert("ldl".into(), vec!["low density lipoprotein".into()]);
    m.insert("hdl".into(), vec!["high density lipoprotein".into()]);

    // Dosing / orders
    m.insert("npo".into(), vec!["nothing by mouth".into()]);
    m.insert("prn".into(), vec!["as needed".into()]);
    m.insert("bid".into(), vec!["twice daily".into()]);
    m.insert("tid".into(), vec!["three times daily".into()]);
    m.insert("qid".into(), vec!["four times daily".into()]);
    m.insert("qd".into(), vec!["once daily".into(), "every day".into()]);

    m
}

/// Returns a mapping of medical phrases to their synonyms / alternative expressions.
fn default_synonyms() -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();

    m.insert("heart attack".into(), vec!["myocardial infarction".into()]);
    m.insert("high blood pressure".into(), vec!["hypertension".into()]);
    m.insert("stroke".into(), vec!["cerebrovascular accident".into()]);
    m.insert("headache".into(), vec!["cephalgia".into()]);
    m.insert("chest pain".into(), vec!["angina".into()]);
    m.insert("blood clot".into(), vec!["thrombosis".into()]);
    m.insert("broken bone".into(), vec!["fracture".into()]);
    m.insert("rash".into(), vec!["dermatitis".into()]);
    m.insert("swelling".into(), vec!["edema".into()]);
    m.insert("dizziness".into(), vec!["vertigo".into()]);
    m.insert("tiredness".into(), vec!["fatigue".into()]);
    m.insert("itching".into(), vec!["pruritus".into()]);
    m.insert("runny nose".into(), vec!["rhinorrhea".into()]);
    m.insert("shortness of breath".into(), vec!["dyspnea".into()]);
    m.insert("low blood sugar".into(), vec!["hypoglycemia".into()]);
    m.insert("high blood sugar".into(), vec!["hyperglycemia".into()]);
    m.insert("kidney failure".into(), vec!["renal failure".into()]);
    m.insert("heart failure".into(), vec!["cardiac failure".into()]);
    m.insert("difficulty swallowing".into(), vec!["dysphagia".into()]);
    m.insert("difficulty breathing".into(), vec!["dyspnea".into()]);
    m.insert("joint pain".into(), vec!["arthralgia".into()]);
    m.insert("muscle pain".into(), vec!["myalgia".into()]);
    m.insert("nausea and vomiting".into(), vec!["emesis".into()]);
    m.insert("loss of appetite".into(), vec!["anorexia".into()]);

    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expander() -> QueryExpander {
        QueryExpander::new()
    }

    #[test]
    fn expands_abbreviation() {
        let e = expander();
        let result = e.expand("patient has htn");
        assert!(result.expanded_terms.iter().any(|t| t == "hypertension"));
        assert!(result.full_query.contains("hypertension"));
    }

    #[test]
    fn expands_synonym_phrase() {
        let e = expander();
        let result = e.expand("high blood pressure management");
        assert!(
            result.expanded_terms.iter().any(|t| t == "hypertension"),
            "expected hypertension in {:?}",
            result.expanded_terms
        );
    }

    #[test]
    fn no_expansion_unknown() {
        let e = expander();
        let result = e.expand("xyzzy frobnicator");
        assert!(result.expanded_terms.is_empty());
        assert_eq!(result.full_query, result.original);
    }

    #[test]
    fn no_duplicate_terms() {
        let e = expander();
        let result = e.expand("htn htn");
        let count = result
            .expanded_terms
            .iter()
            .filter(|t| t.as_str() == "hypertension")
            .count();
        assert_eq!(count, 1, "hypertension should appear exactly once");
    }

    #[test]
    fn expands_multiple() {
        let e = expander();
        let result = e.expand("sob and htn");
        let terms = &result.expanded_terms;
        assert!(
            terms
                .iter()
                .any(|t| t.contains("shortness of breath") || t.contains("dyspnea"))
        );
        assert!(terms.iter().any(|t| t == "hypertension"));
    }

    #[test]
    fn case_insensitive() {
        let e = expander();
        let result = e.expand("HTN");
        assert!(result.expanded_terms.iter().any(|t| t == "hypertension"));
    }

    #[test]
    fn deduped() {
        // "heart attack" as phrase and "mi" both map to "myocardial infarction"
        let e = expander();
        let result = e.expand("mi heart attack");
        let count = result
            .expanded_terms
            .iter()
            .filter(|t| t.as_str() == "myocardial infarction")
            .count();
        assert_eq!(
            count, 1,
            "myocardial infarction should appear exactly once, got {:?}",
            result.expanded_terms
        );
    }
}
