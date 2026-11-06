#!/usr/bin/env bash
# Checks the backend's test coverage against the thresholds CI holds, from
# the summary cargo-llvm-cov writes (just coverage-backend):
#
#   - all crates together: at least 80 % of lines
#   - every bounded context: at least 90 %
#   - the domain layers together: at least 95 %
#
# The thresholds sit just below what the tests reach (11/2026): coverage may
# rise, but not fall unnoticed. Prints the coverage of every crate.
#
# Usage: scripts/coverage-check.sh REPORT.json
set -euo pipefail

report=${1:?usage: $0 REPORT.json}
total_min=80
context_min=90
domain_min=95
contexts=(identity system storage sharing apps containers updates network)

summary=$(jq -r '
  .data[0] as $d
  | ($d.files | map({
      crate: (.filename | capture("/crates/(?<c>[^/]+)/").c),
      domain: (.filename | test("/src/domain(\\.rs|/)")),
      count: .summary.lines.count,
      covered: .summary.lines.covered
    })) as $files
  | (["total", $d.totals.lines.count, $d.totals.lines.covered] | @tsv),
    ($files | map(select(.domain)) | ["domain", (map(.count) | add), (map(.covered) | add)] | @tsv),
    ($files | group_by(.crate)[] | [.[0].crate, (map(.count) | add), (map(.covered) | add)] | @tsv)
' "$report")

# The percentage of a line in the summary: name, lines, covered.
percent() { awk -F'\t' -v name="$1" '$1 == name { printf "%.1f", ($2 ? 100 * $3 / $2 : 100) }' <<<"$summary"; }
below() { awk -v value="$1" -v min="$2" 'BEGIN { exit !(value < min) }'; }

printf '%-28s %8s %8s\n' crate lines covered
awk -F'\t' '$1 ~ /^zendiloos/ { printf "%-28s %8d %7.1f%%\n", $1, $2, 100 * $3 / $2 }' <<<"$summary"

failed=()
total=$(percent total)
domain=$(percent domain)
printf '%-28s %17s%%\n%-28s %17s%%\n' "all crates" "$total" "domain layers" "$domain"
below "$total" "$total_min" && failed+=("all crates: $total % < $total_min %")
below "$domain" "$domain_min" && failed+=("domain layers: $domain % < $domain_min %")
for context in "${contexts[@]}"; do
    value=$(percent "zendiloos-$context")
    [[ -n $value ]] || { failed+=("zendiloos-$context: not in the report"); continue; }
    below "$value" "$context_min" && failed+=("zendiloos-$context: $value % < $context_min %")
done

if (( ${#failed[@]} )); then
    if [[ -t 2 ]]; then red=$'\033[31m' plain=$'\033[0m'; else red='' plain=''; fi
    printf '%sCoverage below the thresholds:%s\n' "$red" "$plain" >&2
    printf '  %s\n' "${failed[@]}" >&2
    exit 1
fi
echo "Coverage holds the thresholds."
