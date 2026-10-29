use std::io::{self, Read};

mod pair_count;

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let mut values = input.split_whitespace().map(|value| value.parse::<i64>().unwrap());
    let n = values.next().unwrap() as usize;
    let threshold = values.next().unwrap();
    let mut numbers: Vec<i64> = values.take(n).collect();
    println!("{}", pair_count::count_pairs(&mut numbers, threshold));
}
