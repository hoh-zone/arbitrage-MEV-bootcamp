use std::cmp::Ordering;

use async_trait::async_trait;

#[async_trait]
pub trait SearchGoal<T, INP, OUT>
where
    INP: Copy
        + Clone
        + std::ops::Add<Output = INP>
        + std::ops::Div<Output = INP>
        + std::ops::Sub<Output = INP>
        + std::ops::Mul<Output = INP>
        + PartialOrd
        + PartialEq
        + Ord
        + Eq
        + std::hash::Hash
        + std::fmt::Debug
        + TryFrom<u128>,
    OUT: Clone,
{
    async fn evaluate(&self, inp: INP, t: &T) -> (INP, OUT);
}

/// This function performs a ternary search to find the maximum value of a
/// function. It works by dividing the search space into three equal parts and
/// discarding one third that contains the least probability of having the
/// answer.
///
/// # Arguments
///
/// * `min` - The lower bound of the search space.
/// * `max` - The upper bound of the search space.
/// * `goal` - The function to maximize. This function should take two
///   arguments: the current input and a mutable context.
/// * `additional_ctx` - Additional context to pass to the `goal` function.
///   Leave it empty if not needed.
///
/// # Return
///
/// This function returns a tuple of three elements:
/// * The input that maximizes the `goal` function.
/// * The maximum value of the `goal` function.
/// * The output of the `goal` function at the maximum input.
///
/// # Type Parameters
///
/// * `T` - The type of the additional context.
/// * `INP` - The type of the input to the `goal` function. Shall be Uint* or
///   number.
pub async fn golden_section_search_maximize<T, INP, OUT>(
    min: INP,
    max: INP,
    goal: impl SearchGoal<T, INP, OUT>,
    additional_ctx: &T,
) -> (INP, INP, OUT)
where
    INP: Copy
        + Clone
        + std::ops::Add<Output = INP>
        + std::ops::Div<Output = INP>
        + std::ops::Sub<Output = INP>
        + std::ops::Mul<Output = INP>
        + PartialOrd
        + PartialEq
        + Ord
        + Eq
        + std::hash::Hash
        + std::fmt::Debug
        + TryFrom<u128>,
    OUT: Clone,
{
    assert!(min < max);

    let one = if let Ok(v) = INP::try_from(1) {
        v
    } else {
        unreachable!("Can't convert 1 to INP type")
    };
    let three = if let Ok(v) = INP::try_from(3) {
        v
    } else {
        unreachable!("Can't convert 2 to INP type")
    };

    // phi = (1.0 + 5.0_f64.sqrt()) / 2.0;
    // phi ~= 1.618033988749895
    // 14566495/9002589 = 1.618033989
    let u: INP = if let Ok(v) = INP::try_from(14566495) {
        v
    } else {
        unreachable!("Can't convert 14566495 to INP type")
    };
    let d: INP = if let Ok(v) = INP::try_from(9002589) {
        v
    } else {
        unreachable!("Can't convert 9002589 to INP type")
    };

    let c = |x: INP| -> INP {
        if x * d < x {
            x / u * d
        } else {
            x * d / u
        }
    };

    let mut left = min;
    let mut right = max;

    let (mut max_in, mut max_f, mut max_out) = {
        let (fl, out_l) = goal.evaluate(left, additional_ctx).await;
        let (fr, out_r) = goal.evaluate(right, additional_ctx).await;
        if fl < fr {
            (right, fr, out_r)
        } else {
            (left, fl, out_l)
        }
    };

    let delta = c(right - left);
    let mut mid_left = right - delta;
    let mut mid_right = left + delta;
    if mid_right <= mid_left {
        mid_right = (mid_left + one).min(right);
    }
    let (mut fl, mut out_left) = goal.evaluate(mid_left, additional_ctx).await;
    if fl > max_f {
        max_f = fl;
        max_in = mid_left;
        max_out = out_left;
    }
    let (mut fr, mut out_right) = goal.evaluate(mid_right, additional_ctx).await;
    if fr > max_f {
        max_f = fr;
        max_in = mid_right;
        max_out = out_right;
    }

    let mut tries = 0;
    while right - left > three && tries < 1000 {
        tries += 1;

        if fl < fr {
            left = mid_left;
            mid_left = mid_right;
            mid_right = left + c(right - left);
            fl = fr;
            (fr, out_right) = goal.evaluate(mid_right, additional_ctx).await;
            if fr > max_f {
                max_f = fr;
                max_in = mid_right;
                max_out = out_right;
            }
        } else {
            right = mid_right;
            let temp = right - c(right - left);
            match temp.cmp(&mid_left) {
                Ordering::Less => {
                    mid_right = mid_left;
                    mid_left = temp;
                    fr = fl;
                    (fl, out_left) = goal.evaluate(mid_left, additional_ctx).await;
                    if fl > max_f {
                        max_f = fl;
                        max_in = mid_left;
                        max_out = out_left;
                    }
                }
                Ordering::Equal => {
                    mid_right = (temp + one).min(right);
                    (fr, out_right) = goal.evaluate(mid_right, additional_ctx).await;
                    if fr > max_f {
                        max_f = fr;
                        max_in = mid_right;
                        max_out = out_right;
                    }
                }
                Ordering::Greater => {
                    mid_right = temp;
                    (fr, out_right) = goal.evaluate(mid_right, additional_ctx).await;
                    if fr > max_f {
                        max_f = fr;
                        max_in = mid_right;
                        max_out = out_right;
                    }
                }
            }
        };
    }

    // Check the inner points, skip the boundaries because we already checked
    for i in 1..=2 {
        let i = if let Ok(v) = INP::try_from(i) {
            v + left
        } else {
            unreachable!("Can't convert {} to INP type", i);
        };
        if i >= right {
            break;
        }
        let (f_mid, out_mid) = goal.evaluate(i, additional_ctx).await;
        if f_mid > max_f {
            max_f = f_mid;
            max_in = i;
            max_out = out_mid;
        }
    }

    (max_in, max_f, max_out)
}

#[cfg(test)]
mod tests {

    use std::collections::HashMap;

    use async_trait::async_trait;

    use super::*;

    #[tokio::test]
    async fn test_golden_section_search1() {
        struct TestGoal;

        #[async_trait]
        impl SearchGoal<(), u32, u32> for TestGoal {
            async fn evaluate(&self, inp: u32, _: &()) -> (u32, u32) {
                let out = inp * 10;
                (out, 0)
            }
        }

        let goal = TestGoal;
        let (input, output, _) = golden_section_search_maximize(1, 9, goal, &()).await;
        println!("gss: input: {}, output: {}", input, output);

        assert_eq!(input, 9);
        assert_eq!(output, 90);
    }

    #[tokio::test]
    async fn test_golden_section_search2() {
        struct TestGoal {
            testdata: HashMap<u128, u128>,
        }

        #[async_trait]
        impl SearchGoal<(), u128, u128> for TestGoal {
            async fn evaluate(&self, inp: u128, _: &()) -> (u128, u128) {
                let out = self.testdata[&inp];
                (out, 0)
            }
        }

        let testdata: HashMap<u128, u128> = HashMap::from_iter([
            (1, 4010106282497016966u128),
            (2, 4418264999713779375u128),
            (3, 4569693292768259346u128),
            (4, 4646875114899946209u128),
            (5, 4691575052709720948u128),
            (6, 4717791501795293046u128),
            (7, 4729882751161429615u128),
            (8, 4724631850822306692u128),
            (9, 4674272470382658763u128),
        ]);

        let goal = TestGoal { testdata };
        let (input, output, _) = golden_section_search_maximize(1u128, 9u128, goal, &()).await;
        println!("gss: input: {}, output: {}", input, output);

        assert_eq!(input, 7);
        assert_eq!(output, 4729882751161429615u128);
    }
}
