/// Unicode-aware subsequence ranking. None means no match; higher ranks first.
pub fn search_score(query: &str, candidate: &str) -> Option<i64> {
    let q = query.trim().to_lowercase();
    let c = candidate.to_lowercase();
    if q.is_empty() {
        return Some(0);
    }
    if q == c {
        return Some(10000);
    }
    if c.starts_with(&q) {
        return Some(8000 - c.chars().count() as i64);
    }
    if c.contains(&q) {
        return Some(6000 - c.chars().count() as i64);
    }
    let mut chars = q.chars();
    let mut next = chars.next();
    let mut cost = 0;
    for (i, ch) in c.chars().enumerate() {
        if Some(ch) == next {
            cost += i as i64;
            next = chars.next();
            if next.is_none() {
                return Some(2000 - cost);
            }
        }
    }
    None
}
