use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::LazyLock;

use serde_json::{json, Value};

use crate::tui::alt_screen_search_index::*;
use crate::tui::utf16::Utf16Text;

static FIXTURE: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../alt_screen_search_index/fixtures.json")).unwrap()
});

fn raw(value: &Value) -> Utf16Text {
    Utf16Text::from_units(
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| u16::try_from(v.as_u64().unwrap()).unwrap())
            .collect(),
    )
}

fn raw_lines(value: &Value) -> Vec<Utf16Text> {
    value.as_array().unwrap().iter().map(raw).collect()
}

fn segment_value(segment: &AltScreenSearchSegment) -> Value {
    json!({"row":segment.row,"startCol":segment.start_col,"endCol":segment.end_col})
}

fn result_value(matches: &SearchMatches) -> Value {
    Value::Array(matches.borrow().iter().map(|m| {
        let m = m.borrow();
        json!({"segments":m.segments.borrow().iter().map(|s| segment_value(&s.borrow())).collect::<Vec<_>>(),"key":get_alt_screen_search_match_key(&m)})
    }).collect())
}

fn corpus_value(corpus: Option<&SearchCorpus>) -> Value {
    corpus.map_or(Value::Null, |c| json!({
        "text":c.text.as_units(),
        "spans":c.spans.iter().map(|s|json!({"textStart":s.text_start,"textEnd":s.text_end,"row":s.row,"startCol":s.start_col,"endCol":s.end_col,"linearColumns":s.linear_columns})).collect::<Vec<_>>()
    }))
}

fn run_group(group: &str, count: usize) {
    let cases = FIXTURE[group].as_array().unwrap();
    assert_eq!(cases.len(), count);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let lines = raw_lines(&case["lines"]);
        let query = raw(&case["query"]);
        let mut index = AltScreenSearchIndex::new();
        let first = index.search_utf16(&lines, &query);
        assert!(first.changed, "{group}/{name} first search");
        let (_, normalized, corpus, _) = index.observed_state();
        let actual = json!({"matches":result_value(&first.matches),"corpus":corpus_value(corpus),"normalizedQuery":normalized.unwrap().as_units()});
        assert_eq!(
            actual, case["expected"],
            "{group}/{name} actual-source oracle"
        );
        assert_eq!(
            result_value(&find_alt_screen_search_matches_utf16(&lines, &query)),
            case["expected"]["matches"],
            "{group}/{name} direct"
        );
        let again = index.search_utf16(&lines.clone(), &query);
        assert!(
            !again.changed,
            "{group}/{name} copied input array must hit cache"
        );
        assert!(
            Rc::ptr_eq(&first.matches, &again.matches),
            "{group}/{name} result array identity"
        );
        if let (Ok(utf8_lines), Ok(utf8_query)) = (
            lines
                .iter()
                .map(Utf16Text::to_string_checked)
                .collect::<Result<Vec<_>, _>>(),
            query.to_string_checked(),
        ) {
            assert_eq!(
                result_value(&find_alt_screen_search_matches(&utf8_lines, &utf8_query)),
                case["expected"]["matches"],
                "{group}/{name} UTF8 convenience"
            );
            let utf8_cached = index.search(&utf8_lines, &utf8_query);
            assert!(!utf8_cached.changed);
            assert!(Rc::ptr_eq(&first.matches, &utf8_cached.matches));
        }
        if let Some(expected) = case.get("graphemes") {
            let actual: Vec<_> = grapheme_ranges(&lines[0])
                .into_iter()
                .map(|range| lines[0].slice(range).into_units())
                .collect();
            assert_eq!(
                json!(actual),
                *expected,
                "Unicode17 conformance and actual Intl boundaries {name}"
            );
        }
    }
}

#[test]
fn search_index_basic_oracle() {
    run_group("basic", 26);
}
#[test]
fn search_index_whitespace_oracle() {
    run_group("whitespace", 87);
}
#[test]
fn search_index_literal_and_terminal_sequences_oracle() {
    run_group("literals", 53);
}
#[test]
fn search_index_unicode_cells_oracle() {
    run_group("unicode", 45);
}
#[test]
fn search_index_all_unicode17_simple_folds_oracle() {
    run_group("folding", 1512);
}
#[test]
fn search_index_unicode17_grapheme_conformance_oracle() {
    run_group("graphemes", 766);
}
#[test]
fn search_index_raw_utf16_oracle() {
    run_group("raw", 185);
}
#[test]
fn search_index_seeded_mixed_input_oracle() {
    run_group("fuzz", 384);
}

#[derive(Default)]
struct Ids {
    ids: HashMap<usize, usize>,
    // Keep every observed allocation alive, including detached/removed objects:
    // pointer reuse must never masquerade as JavaScript object identity.
    keep_alive: Vec<Box<dyn Any>>,
}
impl Ids {
    fn id<T: 'static>(&mut self, value: &Rc<RefCell<T>>) -> usize {
        let address = Rc::as_ptr(value) as usize;
        if let Some(id) = self.ids.get(&address) {
            return *id;
        }
        let id = self.ids.len() + 1;
        self.ids.insert(address, id);
        self.keep_alive.push(Box::new(Rc::clone(value)));
        id
    }
    fn segment(&mut self, s: &SearchSegmentHandle) -> Value {
        let id = self.id(s);
        let mut value = segment_value(&s.borrow());
        value["id"] = json!(id);
        value
    }
    fn segments(&mut self, s: &SearchSegments) -> Value {
        let id = self.id(s);
        let items: Vec<_> = s.borrow().iter().map(|v| self.segment(v)).collect();
        json!({"id":id,"items":items})
    }
    fn one_match(&mut self, m: &SearchMatchHandle) -> Value {
        let id = self.id(m);
        let m = m.borrow();
        let segments = self.segments(&m.segments);
        json!({"id":id,"segments":segments,"key":get_alt_screen_search_match_key(&m)})
    }
    fn matches(&mut self, a: &SearchMatches) -> Value {
        let id = self.id(a);
        let items: Vec<_> = a.borrow().iter().map(|m| self.one_match(m)).collect();
        json!({"id":id,"items":items})
    }
}

struct Nested {
    search_match: SearchMatchHandle,
    segments: SearchSegments,
    segment: SearchSegmentHandle,
}

fn number(v: &Value, key: &str) -> usize {
    usize::try_from(v[key].as_u64().unwrap()).unwrap()
}
fn segment(v: &Value) -> AltScreenSearchSegment {
    AltScreenSearchSegment {
        row: number(v, "row"),
        start_col: number(v, "startCol"),
        end_col: number(v, "endCol"),
    }
}
fn new_match(segments: &Value) -> AltScreenSearchMatch {
    AltScreenSearchMatch::new(segments.as_array().unwrap().iter().map(segment))
}
fn set_segment(s: &SearchSegmentHandle, value: &Value) {
    let mut s = s.borrow_mut();
    if value.get("row").is_some() {
        s.row = number(value, "row");
    }
    if value.get("startCol").is_some() {
        s.start_col = number(value, "startCol");
    }
    if value.get("endCol").is_some() {
        s.end_col = number(value, "endCol");
    }
}

#[test]
fn search_index_cache_and_deep_alias_oracle() {
    let cases = FIXTURE["cache"].as_array().unwrap();
    assert_eq!(cases.len(), 7);
    assert_eq!(
        cases
            .iter()
            .map(|c| c["ops"].as_array().unwrap().len())
            .sum::<usize>(),
        76
    );
    for case in cases {
        let mut index = AltScreenSearchIndex::new();
        let mut current = index.observed_state().3;
        let mut lines = Vec::new();
        let mut ids = Ids::default();
        let mut saved: BTreeMap<String, SearchMatches> = BTreeMap::new();
        let mut nested: BTreeMap<String, Nested> = BTreeMap::new();
        for (step, op) in case["ops"].as_array().unwrap().iter().enumerate() {
            let mut changed = Value::Null;
            let label = op["label"].as_str();
            match op["op"].as_str().unwrap() {
                "lines" => lines = raw_lines(&op["lines"]),
                "line" => lines[number(op, "index")] = raw(&op["text"]),
                "search" => {
                    let r = index.search_utf16(&lines, &raw(&op["query"]));
                    changed = json!(r.changed);
                    current = r.matches;
                }
                "save" => {
                    saved.insert(label.unwrap().to_owned(), Rc::clone(&current));
                }
                "pop" | "reverse" | "push" | "set" => {
                    let array = label.map_or(&current, |label| &saved[label]);
                    match op["op"].as_str().unwrap() {
                        "pop" => {
                            array.borrow_mut().pop();
                        }
                        "reverse" => array.borrow_mut().reverse(),
                        "push" => array
                            .borrow_mut()
                            .push(Rc::new(RefCell::new(new_match(&op["segments"])))),
                        "set" => {
                            let m = Rc::clone(&array.borrow()[number(op, "match")]);
                            let s = Rc::clone(&m.borrow().segments.borrow()[number(op, "segment")]);
                            set_segment(&s, &op["value"]);
                        }
                        _ => unreachable!(),
                    }
                }
                "saveNested" => {
                    let m = Rc::clone(&current.borrow()[number(op, "match")]);
                    let segments = Rc::clone(&m.borrow().segments);
                    let s = Rc::clone(&segments.borrow()[number(op, "segment")]);
                    nested.insert(
                        label.unwrap().to_owned(),
                        Nested {
                            search_match: m,
                            segments,
                            segment: s,
                        },
                    );
                }
                "setNested" => set_segment(&nested[label.unwrap()].segment, &op["value"]),
                "replaceSegments" => {
                    nested[label.unwrap()].search_match.borrow_mut().segments =
                        new_match(&op["segments"]).segments
                }
                "pushNested" => {
                    let n = &nested[label.unwrap()];
                    n.segments.borrow_mut().push(Rc::clone(&n.segment));
                }
                name => panic!("unknown operation {name}"),
            }
            let current_value = ids.matches(&current);
            let saved_value: BTreeMap<_, _> =
                saved.iter().map(|(k, v)| (k, ids.matches(v))).collect();
            let nested_value: BTreeMap<_, _> = nested
                .iter()
                .map(|(k, n)| {
                    let m = ids.one_match(&n.search_match);
                    let segments = ids.segments(&n.segments);
                    let segment = ids.segment(&n.segment);
                    (k, json!({"match":m,"segments":segments,"segment":segment}))
                })
                .collect();
            let (source, query, corpus, _) = index.observed_state();
            let actual = json!({"changed":changed,"current":current_value,"saved":saved_value,"nested":nested_value,"sourceLines":source.map(|v|v.iter().map(Utf16Text::as_units).collect::<Vec<_>>()),"normalizedQuery":query.map(Utf16Text::as_units),"corpus":corpus_value(corpus)});
            assert_eq!(
                actual, case["expected"][step],
                "cache/{} step {step} {op}",
                case["name"]
            );
        }
    }
}

#[test]
fn search_index_match_keys_oracle() {
    let cases = FIXTURE["keys"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for c in cases {
        assert_eq!(
            get_alt_screen_search_match_key(&new_match(&c["segments"])),
            c["expected"].as_str().unwrap(),
            "{}",
            c["name"]
        );
    }
}

#[test]
fn search_index_upstream_named_cross_line() {
    let actual = find_alt_screen_search_matches(&["alpha QUICK", "brown fox"], "quick brown");
    assert_eq!(
        result_value(&actual),
        json!([{"segments":[{"row":0,"startCol":6,"endCol":11},{"row":1,"startCol":0,"endCol":5}],"key":"0:6:1:5"}])
    );
}

#[test]
fn search_index_upstream_named_ansi_and_unicode_width() {
    let actual = find_alt_screen_search_matches(
        &["\x1b[31mfoo  bar\x1b[0m", "A界🙂e\u{301}Z"],
        "bar A界🙂e\u{301}",
    );
    assert_eq!(
        result_value(&actual),
        json!([{"segments":[{"row":0,"startCol":5,"endCol":8},{"row":1,"startCol":0,"endCol":6}],"key":"0:5:1:6"}])
    );
}

#[test]
fn search_index_upstream_named_cache() {
    let mut index = AltScreenSearchIndex::new();
    let first = index.search(&["alpha beta"], "alpha");
    let same = index.search(&["alpha beta"], "alpha");
    let query = index.search(&["alpha beta"], "beta");
    let source = index.search(&["alpha beta gamma"], "gamma");
    assert!(first.changed && !same.changed && query.changed && source.changed);
    assert!(Rc::ptr_eq(&first.matches, &same.matches));
    assert!(!Rc::ptr_eq(&same.matches, &query.matches));
    assert!(!Rc::ptr_eq(&query.matches, &source.matches));
}

#[test]
fn search_index_large_repetitive_literal_is_nonoverlapping() {
    let haystack = "a".repeat(100_000) + "b";
    let query = "a".repeat(20_000) + "b";
    let matches = find_alt_screen_search_matches(&[haystack], &query);
    assert_eq!(matches.borrow().len(), 1);
    assert_eq!(
        get_alt_screen_search_match_key(&matches.borrow()[0].borrow()),
        "0:80000:0:100001"
    );
    let adjacent = find_alt_screen_search_matches(&["aaaaa"], "aa");
    assert_eq!(adjacent.borrow().len(), 2);
}

#[test]
fn search_index_surrogates_are_not_replacements_or_half_pairs() {
    let lines = [Utf16Text::from_units(vec![0xd83d, 0xde42, 0xd83d, 0xfffd])];
    let high = find_alt_screen_search_matches_utf16(&lines, &Utf16Text::from_units(vec![0xd83d]));
    assert_eq!(high.borrow().len(), 1);
    assert_eq!(
        get_alt_screen_search_match_key(&high.borrow()[0].borrow()),
        "0:2:0:2"
    );
    let replacement = find_alt_screen_search_matches_utf16(&lines, &Utf16Text::from("\u{fffd}"));
    assert_eq!(replacement.borrow().len(), 1);
    assert_eq!(
        get_alt_screen_search_match_key(&replacement.borrow()[0].borrow()),
        "0:2:0:3"
    );
}

#[test]
fn search_index_case_is_a_cache_change_but_normalized_whitespace_is_not() {
    let mut index = AltScreenSearchIndex::new();
    let first = index.search(&["a b"], "a b");
    let whitespace = index.search(&["a b"], " \ta\n b\u{feff}");
    let case = index.search(&["a b"], "A B");
    assert!(!whitespace.changed && case.changed);
    assert!(Rc::ptr_eq(&first.matches, &whitespace.matches));
    assert!(!Rc::ptr_eq(&first.matches, &case.matches));
    assert_eq!(result_value(&first.matches), result_value(&case.matches));
}

#[test]
fn search_index_detached_nested_aliases_survive_recomputation() {
    let mut index = AltScreenSearchIndex::new();
    let first = index.search(&["a"], "a");
    let old_match = Rc::clone(&first.matches.borrow()[0]);
    let old_segments = Rc::clone(&old_match.borrow().segments);
    let old_segment = Rc::clone(&old_segments.borrow()[0]);
    old_match.borrow_mut().segments = SearchSegments::default();
    old_segment.borrow_mut().row = 7;
    let cached = index.search(&["a"], "a");
    assert!(Rc::ptr_eq(&first.matches, &cached.matches));
    assert_eq!(get_alt_screen_search_match_key(&old_match.borrow()), "");
    let replaced = index.search(&["aa"], "a");
    assert!(!Rc::ptr_eq(&first.matches, &replaced.matches));
    assert_eq!(old_segments.borrow()[0].borrow().row, 7);
    assert_eq!(replaced.matches.borrow().len(), 2);
    assert_eq!(
        get_alt_screen_search_match_key(&replaced.matches.borrow()[0].borrow()),
        "0:0:0:1"
    );
}
