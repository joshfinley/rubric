# TMR voter failure-mode analysis

External evidence for `TMR-VOTE`. The voter is a 2-of-3 majority over three
independently developed channels.

| Failure | Effect | Mitigation |
|---|---|---|
| One channel stuck high | Out-voted by the other two | Majority absorbs a single fault |
| One channel stuck low | Out-voted by the other two | Majority absorbs a single fault |
| Two channels agree wrongly | Wrong output | Channel diversity makes correlated faults unlikely |

This file is content-sealed under the `file:` scheme. Editing it breaks the
`TMR-VOTE` seal until the change is reviewed, accepted, and re-attested.
