# KeyValues (VDF)

| | |
|---|---|
| Status | Implemented (M0): text and binary, read and write |
| Code | `crates/ossteam-vdf` |

KeyValues is the tree format Steam uses for app manifests (`.acf`),
library and configuration files (`.vdf`), install scripts, tool manifests,
and PICS app info. A tree is a list of entries. An entry has a string key
and a value, and the value is either a scalar or a nested list. In this
page, "list" means an ordered sequence in which keys may repeat. Readers
must keep both the order and the duplicates, because real files depend on
both.

Steam looks keys up without regard to ASCII case, and so does
`ossteam-vdf`: `Object::get` returns the first match, and `get_all`
returns every match in order.

## 1. Text form

```
// a comment runs to the end of the line
"AppState"
{
    "appid"      "480"
    "installdir" "Spacewar"   [$WIN32]
    unquoted     tokens
}
```

**Tokens.**
- A *quoted string* runs from `"` to the next unescaped `"`. It may
  contain newlines.
- An *unquoted string* runs until whitespace, `"`, `{`, `}` or `//`.
- `{` and `}` delimit a nested list.
- `[` … `]` (on one line) is a *conditional* (§1.2).
- `//` starts a comment that runs to the end of the line.
- A UTF-8 byte-order mark at the start is ignored.

**Escapes.** Inside quoted strings the escapes are `\\`, `\"`, `\n`, `\t`
and `\r`. Any other backslash is kept as written, together with the
character after it. Files written by older tools often contain
unescaped Windows paths such as `C:\Games\x`, and this rule keeps them
intact. The writer always escapes the five characters above.

**Grammar.**

```
file      = { directive | entry }
entry     = key [cond] ( value [cond] | [cond] "{" { entry } "}" )
key       = string
value     = string
directive = ("#include" | "#base") string    ; top level only
```

A file may have several top-level entries, and all of them are kept.

### 1.1 Directives

`#include "f"` adds the entries of file `f` alongside this file's.
`#base "f"` supplies entries from `f` that this file does not define. The
parser records directives in `Document::directives` and does **not** read
the files they name: reading other files is the caller's decision. Plain
`text::parse` rejects directives with `UnknownDirective`. None of the
Steam files that ossteam reads in M1 use them.

### 1.2 Conditionals

A conditional such as `[$WIN32]`, `[!$X360]` or `[$WIN32||$OSX]` limits an
entry to some platforms. It can follow a scalar value, or come between a
key and its `{`. The parser stores the condition without its brackets in
`Entry::condition` and does not evaluate it. Callers that care, such as
launch-option or install-script handling, evaluate it.

### 1.3 Writer layout

The writer produces the layout Steam writes: quoted keys and values, one
tab of indentation per level, braces on their own lines, and two tabs
between a key and its value. A conditional is written after the value, or
after the key of a nested list. Floats are written with six decimals.

## 2. Binary form

The binary form is a stream of entries. Each entry is a one-byte type tag,
a key, and then a value whose layout depends on the tag. All integers are
little-endian.

| Tag | Meaning | Value |
|---|---|---|
| `0x00` | nested list | entries up to an end tag |
| `0x01` | string | UTF-8 bytes, NUL-terminated |
| `0x02` | int32 | 4 bytes |
| `0x03` | float32 | 4 bytes |
| `0x04` | pointer | 4 bytes, kept only for round trips |
| `0x05` | wide string | UTF-16LE, ended by a 16-bit NUL |
| `0x06` | colour | 4 bytes (RGBA) |
| `0x07` | uint64 | 8 bytes |
| `0x08` | end of list | no key and no value |
| `0x0A` | int64 | 8 bytes |
| `0x0B` | end of list (alternate) | no key and no value |

**Keys.** A key is either an inline NUL-terminated UTF-8 string or, in the
*string-table* variant, a `u32` index into a table of key strings that the
containing file stores elsewhere. Newer app info caches use the
string-table variant. Callers pass `Keys::Table(&table)` to the parser and
use `to_vec_with_table` to write.

**Ends.** A nested list must end with an end tag. The top-level list ends
either with an end tag or at the end of the input, because some writers
leave out the final tag. `binary::parse` returns how many bytes it
consumed, so callers can parse several lists stored back to back.

**Writing.** Text conditionals have no binary form and are dropped when
writing binary. A string that contains a NUL byte is cut off at that byte,
so that what is written is exactly what a reader will see.

## 3. Robustness (SEC-5)

Both parsers reject nesting deeper than 256 levels and report the position
of every error: line and column for text, byte offset for binary. Binary
strings must be valid UTF-8, or valid UTF-16 for wide strings. Invalid
input is an error and is never replaced with lossy characters. Both
parsers are fuzz targets (`fuzz/`), which also check that every parsed
tree survives a write and re-parse.
