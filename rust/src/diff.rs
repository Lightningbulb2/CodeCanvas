//! Line diff: LCS with the common prefix and suffix trimmed.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op { Same(usize, usize), Del(usize), Add(usize) }

pub fn diff_lines(a: &[String], b: &[String]) -> Vec<Op> {
    let p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let max = a.len().min(b.len()) - p;
    let s = a.iter().rev().zip(b.iter().rev()).take(max).take_while(|(x, y)| x == y).count();
    let (aa, bb) = (&a[p..a.len() - s], &b[p..b.len() - s]);
    let (n, m) = (aa.len(), bb.len());
    let mut ops: Vec<Op> = (0..p).map(|i| Op::Same(i, i)).collect();
    if n * m > 250_000 {
        ops.extend((0..n).map(|i| Op::Del(p + i)));
        ops.extend((0..m).map(|j| Op::Add(p + j)));
    } else {
        let mut dp = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[i][j] = if aa[i] == bb[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if aa[i] == bb[j] { ops.push(Op::Same(p + i, p + j)); i += 1; j += 1; }
            else if dp[i + 1][j] >= dp[i][j + 1] { ops.push(Op::Del(p + i)); i += 1; }
            else { ops.push(Op::Add(p + j)); j += 1; }
        }
        ops.extend((i..n).map(|k| Op::Del(p + k)));
        ops.extend((j..m).map(|k| Op::Add(p + k)));
    }
    ops.extend((0..s).map(|k| Op::Same(a.len() - s + k, b.len() - s + k)));
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn finds_minimal_changes() {
        let ops = diff_lines(&s(&["a", "b", "c"]), &s(&["a", "x", "c", "d"]));
        assert_eq!(ops, vec![Op::Same(0, 0), Op::Del(1), Op::Add(1), Op::Same(2, 2), Op::Add(3)]);
    }

    #[test]
    fn identical_has_no_edits() {
        assert!(diff_lines(&s(&["a", "b"]), &s(&["a", "b"])).iter().all(|o| matches!(o, Op::Same(..))));
    }
}
