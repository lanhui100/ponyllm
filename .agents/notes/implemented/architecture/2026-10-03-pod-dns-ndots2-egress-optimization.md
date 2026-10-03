# Agent Note: Pod DNS ndots2 Optimization for Egress Probe Timeout

Status: implemented

## Problem
In Kubernetes, default `/etc/resolv.conf` sets `ndots:5` with multiple cluster search domains (`ponyllm.svc.cluster.local`, `svc.cluster.local`, `cluster.local`). When probing or resolving external FQDN targets such as `daily-cloudcode-pa.googleapis.com` (which has 3 dots), `getaddrinfo` sequentially attempts cluster search domain suffixes for both A and AAAA records, causing DNS lookup duration to reach 10 seconds. This exceeds ponyllm's egress policy fail-closed probe timeout (5 seconds), causing false positive `egress_blocked` errors.

## Decision
Configure `dnsConfig.options: [{name: "ndots", value: "2"}]` across all ponyllm gateway Deployments in `deploy/ponyllm-deployment.yaml`. Since public external domains typically have 2 or more dots, setting `ndots: 2` allows `getaddrinfo` to query the root/upstream DNS immediately without traversing cluster search domains, reducing resolution time from 10s to <10ms.

## Alternatives considered
- *Increase egress probe timeout in ponyllm code to 15s*: Only masks slow DNS queries, prolonging latency for failed probes and degrading user experience during key verification and egress probing.
- *Append trailing dot to all configured endpoints (`https://domain.`)*: Requires manual operational vigilance and breaks typical user configuration conventions.
