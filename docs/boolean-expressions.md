# Boolean expressions

`Bool` has two values: `true` and `false`. Conditions and logical operators
require `Bool`; numbers, strings, `Any`, and optional booleans have no implicit
truth-value conversion.

```hks
var finished: Bool = false
var count = 0

while !finished && count < 4 {
    count += 1
    finished = count == 3
}

let visible = !(finished || count == 0)
let optional: Bool? = null
let enabled = optional ?: false
```

Precedence, highest first: postfix member/call/non-null operations, prefix `!`
and `-`, multiplication/division, addition/subtraction, ordered comparisons,
equality (`==`, `!=`), `&&`, `||`, then `?:`. Parentheses override precedence:
`(expression)` is grouping, `(expression,)` is a one-element tuple, and `()` is
Unit.

`&&` skips its right operand when the left is false. `||` skips its right
operand when the left is true. Both operands are type-checked even if one will
be skipped. Short-circuit branches can suspend at native calls and survive VM
snapshot/restore. Scalar constants and template expressions also support
logical operators.

Local optional values narrow on the appropriate path:

```hks
fn positive(value: Int) -> Bool { value > 0 }
let score: Int? = 12
let accepted = score != null && positive(score)
if !(score == null) {
    let result = positive(score)
}
```
