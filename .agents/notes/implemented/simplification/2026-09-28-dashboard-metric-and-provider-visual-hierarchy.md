# Agent Note: Dashboard metric and provider visual hierarchy

Status: implemented

## Problem
The dashboard mixes explanatory period copy, colored metric icons, and colored provider data values in dense telemetry cards. This makes the primary value hierarchy harder to scan and leaves the Antigravity pool's dense visualizations top-heavy inside equal-height cards. Provider latency labels also use decimal badge styling while table headers lack semantic visual cues.

## Decision
The dashboard presents the selected metric range through the existing segmented control without repeating the current range beside the section title. Metric card title icons inherit the title color, token dimensions use semantic icons paired with neutral values, and the input/output/cache icons reuse the emerald/amber/sky semantic colors from the Antigravity quota breakdown; latency/rate cards keep only the TTFT/TPS labels, and throughput units use `t/s`. Antigravity availability and Gemini quota cards use explicit top-aligned heading blocks (`shrink-0`) with a separate flexible body (`flex-1`) that vertically centers only the matrix/progress visualization; explanatory footers remain anchored at the bottom. Provider status removes decorative header/table backgrounds, adds semantic icons to every column heading, renders provider latency as an integer in a fixed-width neutral text slot without a badge background, and reserves color for connectivity bars/latency, with error values always red and other data neutral.

## Alternatives considered
- Keep colored token dimensions and provider columns: rejected because color competed with the connectivity status signal and made the table harder to scan.
- Remove all labels from latency and rate cards: rejected because TTFT and TPS remain necessary semantic anchors when the explanatory copy is removed.
- Center the entire Antigravity card including footers: rejected because the footer guidance should remain anchored to the bottom while the primary visualization is centered in the available body.
- Use decimal latency values with a colored badge: rejected because the requested integer, fixed-width display and no-badge treatment better supports column alignment.

## Consequences
The dashboard is denser and more neutral by default, with color reserved for status semantics. Existing telemetry behavior and data calculations remain unchanged. Tests that assert user-facing labels now validate the updated `24小时`, `TTFT`, `TPS`, `t/s`, and provider column naming.
