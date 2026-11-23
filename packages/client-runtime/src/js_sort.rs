//! `Array.prototype.sort` as V8 runs it, for comparators that can be
//! inconsistent. Host dates that do not parse compare as `NaN`, which
//! JavaScript reads as "equal", so the order then depends on the algorithm.
//! V8's TimSort sorts arrays shorter than 64 by finding the first run
//! (reversing a strictly descending one) and binary-inserting the rest; that
//! is ported exactly. Longer arrays fall back to Rust's stable sort, which
//! agrees whenever the comparator is consistent.

use std::cmp::Ordering;

/// Sorts `items` in place the way V8 would with `compare`.
pub fn js_sort_by<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
    let len = items.len();
    if len < 2 {
        return;
    }
    if len >= 64 {
        items.sort_by(compare);
        return;
    }
    // CountAndMakeRun.
    let mut run = 2;
    let descending = compare(&items[1], &items[0]) == Ordering::Less;
    while run < len {
        let order = compare(&items[run], &items[run - 1]);
        if descending {
            if order != Ordering::Less {
                break;
            }
        } else if order == Ordering::Less {
            break;
        }
        run += 1;
    }
    if descending {
        items[..run].reverse();
    }
    // BinaryInsertionSort.
    for start in run..len {
        let (mut left, mut right) = (0, start);
        while left < right {
            let mid = left + ((right - left) >> 1);
            if compare(&items[start], &items[mid]) == Ordering::Less {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        items[left..=start].rotate_right(1);
    }
}

/// The order a JavaScript comparator returning `left - right` gives; `NaN` is equal.
pub fn js_sub_order(left: f64, right: f64) -> Ordering {
    let d = left - right;
    if d < 0.0 {
        Ordering::Less
    } else if d > 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}
