# Cua: experimental typed Lua

`emmylua_compile` checks a typed Lua source file and emits ordinary Lua. Type
checking uses the existing EmmyLua semantic engine and `.d.lua` libraries.
The compiler and language server share the Cua frontend. Ordinary `.lua` files
continue to use the existing Lua parser.

```lua
---@alias CameraName string

local enabled: boolean = false

local function cameraName(name: CameraName?): string
    return name or "orbit"
end

local function position(): (number, number)
    return 0, 0
end
```

```sh
cargo run -p emmylua_transpiler -- camera.cua -o camera.lua
cargo run -p emmylua_transpiler -- camera.cua --check
cargo run -p emmylua_transpiler -- camera.cua -o camera.lua --lua-version luajit -L path/to/common -L path/to/ge
```

`-L` accepts library files or directories and may be repeated. Libraries are
indexed without checking their implementations. Without `--workspace`, the compiler enables
`noImplicitAny` and `checkUnknown` for the source. Errors and warnings prevent
output; hints do not. An existing output file is only replaced after a successful
check. The source file cannot be its own output.

The emitted Lua preserves original line breaks and byte offsets. Inline type
text becomes spaces; expressions, comments and string literals remain intact.
Diagnostics refer to the original source. Internally generated annotations are
only input to the checker and do not appear in emitted Lua.

## Supported in this prototype

- Local variable types: `local value: number = 1`.
- Fully annotated multiple locals: `local x: number, y: string = 1, "a"`.
- Parameters of named functions and methods, including `...: number`.
- Single return types and parenthesized return lists.
- Expression assertions: `value as Type`, with parentheses for subsequent field access or calls.
- EmmyLua type expressions, including `Camera?`, unions, arrays, object shapes,
  `table<string, Camera?>` and `fun(x: number): string`.
- Types declared by existing EmmyLua annotations and external libraries.
- Unannotated Lua remains unchanged and participates in ordinary inference.

## Current limits

Each type expression must fit on one physical line. Split declarations mixing
typed and inferred locals into separate statements. Inline types on anonymous
functions are rejected; use named functions for now. Do not declare the same
parameter, return contract or variable type both inline and in an annotation;
duplicate declarations are rejected, even if their text matches.

The command emits one source file. Use `--workspace <root>` to load project
modules, `.emmyrc` settings and configured libraries; `.cua` modules resolve
through ordinary `require` names. Workspace strict settings and Lua version
are preserved; `--lua-version` overrides the configured version. Building all
project files remains the responsibility of the build script. Imports are not rewritten.
Cua (Cats + Lua) sources use the `.cua` extension. Aliases, classes, generics
on declarations, named flow casts (`---@cast`) and overload declarations still use EmmyLua annotations.
This is type erasure, not a runtime validator, and all existing analyzer limits
still apply. Lua 5.1 is the default target; other versions require `--lua-version`.

## Editor support

This version of `emmylua_ls` indexes `.cua` files automatically. Diagnostics and
hover use positions in the original source, including inline types and Unicode.
The server uses the project's normal `.emmyrc` configuration and libraries.
An incomplete declaration produces a Cua syntax diagnostic; semantic hover is
unavailable for that file until it parses again.

In VS Code with the EmmyLua extension, associate `.cua` with Lua and select the
built server in user or workspace settings:

```json
{
  "files.associations": { "*.cua": "lua" },
  "emmylua.ls.executablePath": "D:/DevTools/emmylua/emmylua_ls.exe"
}
```

Use your own executable path. Run **EmmyLua: Restart Lua Server** after updating
the binary. Cua uses the same semantic symbol classifications and modifiers as
Lua; inline types after `:` and `as` share the same highlighting. Exact colors
and decorations depend on the editor theme and EmmyLua color settings.
EmmyLua variable colors and mutable-variable underlines also apply to Cua.
Cua supports Go to Definition for
symbols, inline types and imports, and Go to
Type Definition for values with named types. Targets in Lua libraries and Cua
sources use their original source positions. Parameter name hints
are available with positions mapped to the original source.
Completion, find references
and formatting are not enabled yet. Rename and workspace
symbol search are also disabled in mixed Lua/Cua workspaces until their
cross-file ranges support Cua; generated Lua positions must never become edits
to the source.

## Casts

`expression as Type` asserts the type of the preceding expression, with the same
intent as `--[[@as Type]]`. Parentheses are optional when the cast finishes the
expression (an initializer, argument, table value or return value). Use
`(expression as Type)` before subsequent field access, calls or operators.
The target supports named types, unions, nullable types, object shapes, generic
tables and function types. Write unions directly: `(value as string|number)`.

```lua
local label = options[index] as string
local position = (value as Camera).position
local result = (callback as fun(): number)()
```

A cast performs no runtime conversion or validation. It does not change the
variable's type for later reads. The emitted Lua retains only parentheses explicitly present in the source.
Assertions can override an incompatible type; subsequent uses are checked
against the asserted type. Unknown type names still produce errors.
