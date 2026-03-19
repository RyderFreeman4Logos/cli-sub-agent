#!/usr/bin/env bash
# crate-topo.sh — Extract workspace crate topological order using Kahn's algorithm.
# Output: comma-separated crate names in dependency order (leaves first).
# Exit 1 if circular dependency detected.
set -euo pipefail

JQ_FILTER='
  # Collect workspace package names
  [.packages[] | .name] as $ws_names |

  # Build graph: [{name, deps}]
  [.packages[] | {
    name: .name,
    deps: [.dependencies[] | select(.path != null) | .name | select(. as $d | $ws_names | index($d))]
  }] |

  # Kahn topological sort
  # Edge direction: if A depends on B, edge is B->A (B must come before A).
  # So in_degree[A] = len(A.deps).
  (reduce .[] as $node (
    {};
    . + {($node.name): ($node.deps | length)}
  )) as $initial_in_degree |

  . as $graph |

  # BFS with until
  {
    queue: ([$graph[] | select(($initial_in_degree[.name] // 0) == 0) | .name] | sort),
    result: [],
    in_degree: $initial_in_degree,
    processed: 0,
    graph: $graph
  } |
  until(.queue | length == 0;
    .queue[0] as $current |
    .result += [$current] |
    .queue = .queue[1:] |
    .processed += 1 |
    reduce (.graph[] | select(.deps | index($current)) | .name) as $neighbor (
      .;
      .in_degree[$neighbor] = (.in_degree[$neighbor] - 1) |
      if .in_degree[$neighbor] == 0 then .queue += [$neighbor] else . end
    )
  ) |

  if .processed != (.graph | length) then
    "ERROR:CYCLE:" + ([.graph[] | .name] - .result | join(","))
  else
    .result | join(",")
  end
'

RESULT=$(cargo metadata --no-deps --format-version 1 2>/dev/null | jq -r "$JQ_FILTER")

if [[ "$RESULT" == ERROR:CYCLE:* ]]; then
  CYCLE_NODES="${RESULT#ERROR:CYCLE:}"
  echo "ERROR: Circular dependency detected among: ${CYCLE_NODES}" >&2
  exit 1
fi

echo "$RESULT"
