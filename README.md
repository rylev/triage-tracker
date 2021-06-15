# Triage Tracker

A small utility for tracking the change in opening and closing of issues in a GitHub repo.

This is currently hardcoded for use with [rust-lang/rust](https://github.com/rust-lang/rust).

## Use 

To get stats for a range of days:

```bash
cargo run -- -s 2021-06-07 -e 2021-05-31
```

To get stats for a particular day:

```bash
cargo run -- -d 2021-06-07
```