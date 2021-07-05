# Triage Tracker

A small utility for helping with triaging GitHub issues. 

Currently this utility has two main pieces of functionality:
* tracking the change in net closing of issues in a GitHub repo. 
* tracking issues that have not been commented since a certain time.

This tool can be used to build visualizations for issue triage over time with the hope of motivating closing more issues than are opened.

This is currently hardcoded for use with [rust-lang/rust](https://github.com/rust-lang/rust).

## Use 

### Net issue closings

To get net issue closings for a range of days:

```bash
triage-tracker closings range -s 2021-06-07 -e 2021-05-31
```

To get net issue closings for a particular day:

```bash
triage-tracker closings date 2021-06-07 
```

### Stale issues

To see issues that have not been commented on since a certain date that are tagged with a certain tag:

```bash
triage-tracker triaged A-diagnostics --since 2021-07-01  
```

The tags and `since` date are both optional. If no `since` date is provided, one year before the present day is used.
