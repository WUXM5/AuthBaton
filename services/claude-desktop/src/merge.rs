// 派生自 cc-same crates/core/src/merge.rs（v0.1.12, MIT, Songkeys）
//! Three-way merge of Desktop's JSON collection files (`scheduled-tasks.json`,
//! `backlog/tasks.json`).
//!
//! Each member's current file is compared with what we last synced to that member (its
//! base) and with the group's last merged result:
//! * a member's edits since its base win, newest file first; removals count as edits;
//! * without edits the group's last result stands;
//! * a member that has never been synced can add items, never remove or overwrite them;
//! * the very first merge is a union (newest file wins on conflicts).
//!
//! Lists of objects with an `id` merge item by item; objects merge key by key.
//!
//! AuthManager: without serde_json's `preserve_order` an object's keys come out sorted, which
//! JSON does not mind; a list keeps its items' order (see `merge_id_list`).

use serde_json::{Map, Value};

/// A member's base for some key: never synced, synced without the key, or synced with it.
#[derive(Clone, Copy, Debug)]
enum Base<'a> {
    Never,
    Missing,
    Val(&'a Value),
}

type Slot<'a> = Option<&'a Value>;

fn same(cur: Slot<'_>, base: Base<'_>) -> bool {
    match (cur, base) {
        (None, Base::Missing) => true,
        (Some(a), Base::Val(b)) => a == b,
        _ => false,
    }
}

fn merge_scalar(curs: &[(Slot<'_>, Base<'_>)], prev: Slot<'_>, have_prev: bool) -> Option<Value> {
    for &(cur, base) in curs {
        if !matches!(base, Base::Never) && !same(cur, base) {
            return cur.cloned(); // a real edit since this member's last sync (None = removed)
        }
    }
    if have_prev {
        if let Some(p) = prev {
            return Some(p.clone());
        }
        // Absent from the group: only a never-synced member may add it.
        return curs.iter().find(|(c, b)| matches!(b, Base::Never) && c.is_some()).and_then(|(c, _)| c.cloned());
    }
    curs.iter().find_map(|(c, _)| c.cloned()) // first merge ever: union, newest file first
}

fn sub_base<'a>(base: Base<'a>, key: &str) -> Base<'a> {
    match base {
        Base::Never => Base::Never,
        Base::Val(Value::Object(m)) => m.get(key).map_or(Base::Missing, Base::Val),
        _ => Base::Missing,
    }
}

fn merge_mapping(curs: &[(Slot<'_>, Base<'_>)], prev: Slot<'_>, have_prev: bool) -> Map<String, Value> {
    let mut keys: Vec<String> = Vec::new();
    for v in curs.iter().map(|(c, _)| *c).chain(std::iter::once(prev)) {
        if let Some(Value::Object(m)) = v {
            for k in m.keys() {
                if !keys.contains(k) {
                    keys.push(k.clone());
                }
            }
        }
    }
    let mut out = Map::new();
    for k in &keys {
        let sub: Vec<(Slot<'_>, Base<'_>)> = curs
            .iter()
            .map(|(cur, base)| (cur.and_then(|c| c.as_object()).and_then(|m| m.get(k)), sub_base(*base, k)))
            .collect();
        let pv = prev.and_then(|p| p.as_object()).and_then(|m| m.get(k));
        if let Some(v) = merge_scalar(&sub, pv, have_prev) {
            out.insert(k.clone(), v);
        }
    }
    out
}

fn is_id_list(v: &Value) -> bool {
    match v {
        Value::Array(items) => items.iter().all(|x| {
            x.as_object().and_then(|o| o.get("id")).is_some_and(|id| id.is_string() || id.is_i64() || id.is_u64())
        }),
        _ => false,
    }
}

/// `[{id, …}, …]` → `{"<id json>": item}` preserving order.
fn by_id(v: &Value) -> Value {
    let mut m = Map::new();
    if let Value::Array(items) = v {
        for item in items {
            if let Some(id) = item.get("id") {
                m.insert(id.to_string(), item.clone());
            }
        }
    }
    Value::Object(m)
}

fn merge_id_list(curs: &[(Slot<'_>, Base<'_>)], prev: Slot<'_>, have_prev: bool) -> Value {
    let cur_maps: Vec<Option<Value>> = curs.iter().map(|(c, _)| c.map(by_id)).collect();
    let base_maps: Vec<Option<Value>> = curs
        .iter()
        .map(|(_, b)| match b {
            Base::Val(v) => Some(by_id(v)),
            _ => None,
        })
        .collect();
    let prev_map = prev.map(by_id);
    let pairs: Vec<(Slot<'_>, Base<'_>)> = curs
        .iter()
        .enumerate()
        .map(|(i, (_, b))| {
            let base = match b {
                Base::Never => Base::Never,
                Base::Missing => Base::Missing,
                Base::Val(_) => Base::Val(base_maps[i].as_ref().unwrap()),
            };
            (cur_maps[i].as_ref(), base)
        })
        .collect();
    let mut merged = merge_mapping(&pairs, prev_map.as_ref(), have_prev);
    // AuthManager: serde_json's `Map` sorts its keys in this workspace (no `preserve_order`), so
    // the merged items go back in the order cc-same's ordered map kept them: as first seen in the
    // members' lists (newest file first), then in the group's last result.
    let mut order: Vec<String> = Vec::new();
    for list in curs.iter().map(|(c, _)| *c).chain(std::iter::once(prev)) {
        for item in list.and_then(Value::as_array).into_iter().flatten() {
            if let Some(id) = item.get("id").map(Value::to_string) {
                if !order.contains(&id) {
                    order.push(id);
                }
            }
        }
    }
    Value::Array(order.iter().filter_map(|id| merged.remove(id)).collect())
}

/// Merge one collection file. `members`: (current content, base) newest file first.
pub fn merge_collection(members: &[(&Map<String, Value>, Option<&Value>)], prev: Option<&Value>) -> Map<String, Value> {
    let have_prev = prev.is_some();
    let prev_obj = prev.and_then(|p| p.as_object());
    let mut keys: Vec<String> = Vec::new();
    for (cur, _) in members {
        for k in cur.keys() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    if let Some(p) = prev_obj {
        for k in p.keys() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
    }
    let mut out = Map::new();
    for k in &keys {
        let curs: Vec<(Slot<'_>, Base<'_>)> = members
            .iter()
            .map(|(cur, base)| {
                let b = match base {
                    None => Base::Never,
                    Some(v) => sub_base(Base::Val(v), k),
                };
                (cur.get(k), b)
            })
            .collect();
        let pv = prev_obj.and_then(|p| p.get(k));
        let mut vals: Vec<&Value> = curs.iter().filter_map(|(c, _)| *c).collect();
        vals.extend(curs.iter().filter_map(|(_, b)| if let Base::Val(v) = b { Some(*v) } else { None }));
        vals.extend(pv);
        let merged = if !vals.is_empty() && vals.iter().all(|v| is_id_list(v)) {
            Some(merge_id_list(&curs, pv, have_prev))
        } else if !vals.is_empty() && vals.iter().all(|v| v.is_object()) {
            Some(Value::Object(merge_mapping(&curs, pv, have_prev)))
        } else {
            merge_scalar(&curs, pv, have_prev)
        };
        if let Some(v) = merged {
            out.insert(k.clone(), v);
        }
    }
    out
}

/// Nothing worth creating a file for: every list/object inside is empty.
pub fn trivially_empty(v: &Map<String, Value>) -> bool {
    v.values().all(|x| match x {
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => trivially_empty(o),
        _ => true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn first_merge_is_a_union_with_newest_winning() {
        let a = obj(json!({"a": 1, "l": [{"id": 1, "v": "new"}]}));
        let b = obj(json!({"a": 2, "l": [{"id": 1, "v": "old"}, {"id": 2}]}));
        let r = merge_collection(&[(&a, None), (&b, None)], None);
        assert_eq!(Value::Object(r), json!({"a": 1, "l": [{"id": 1, "v": "new"}, {"id": 2}]}));
    }

    #[test]
    fn a_members_edit_wins_even_when_another_file_is_newer() {
        let prev = json!({"a": 1, "l": [{"id": 1, "v": "new"}, {"id": 2}]});
        let unchanged = obj(prev.clone());
        let edited = obj(json!({"a": 5, "l": [{"id": 1, "v": "new"}, {"id": 2}]}));
        let r = merge_collection(&[(&unchanged, Some(&prev)), (&edited, Some(&prev))], Some(&prev));
        assert_eq!(r["a"], json!(5));
    }

    #[test]
    fn a_never_synced_member_cannot_remove_items() {
        let prev = json!({"a": 1, "l": [{"id": 1}, {"id": 2}]});
        let newcomer = obj(json!({"a": 1, "l": []}));
        let member = obj(prev.clone());
        let r = merge_collection(&[(&newcomer, None), (&member, Some(&prev))], Some(&prev));
        assert_eq!(r["l"], json!([{"id": 1}, {"id": 2}]));
    }

    /// Ids that sort differently from the list (`"b" < "c"`, `10 < 9` as text): the list order
    /// stays.
    #[test]
    fn merged_lists_keep_their_order() {
        let a = obj(json!({"l": [{"id": "c"}, {"id": "a"}, {"id": 10}]}));
        let b = obj(json!({"l": [{"id": "c"}, {"id": "b"}, {"id": 9}]}));
        let r = merge_collection(&[(&a, None), (&b, None)], None);
        assert_eq!(r["l"], json!([{"id": "c"}, {"id": "a"}, {"id": 10}, {"id": "b"}, {"id": 9}]));
        let prev = Value::Object(r.clone());
        let r = merge_collection(&[(&r, Some(&prev)), (&r, Some(&prev))], Some(&prev));
        assert_eq!(Value::Object(r), prev);
    }

    #[test]
    fn an_item_removed_from_the_group_does_not_come_back() {
        // A removed t2 and received the merge; B has not received it yet.
        let prev = json!({"l": [{"id": "t1"}]});
        let a = obj(json!({"l": [{"id": "t1"}]}));
        let b = obj(json!({"l": [{"id": "t1"}, {"id": "t2"}]}));
        let b_base = json!({"l": [{"id": "t1"}, {"id": "t2"}]});
        let r = merge_collection(&[(&a, Some(&prev)), (&b, Some(&b_base))], Some(&prev));
        assert_eq!(r["l"], json!([{"id": "t1"}]));
    }
}
