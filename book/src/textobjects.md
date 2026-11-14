## Selecting and manipulating text with textobjects

In Helix, textobjects are a way to select, manipulate and operate on a piece of
text in a structured way. They allow you to refer to blocks of text based on
their structure or purpose, such as a word, sentence, paragraph, or even a
function or block of code.

![Textobject demo](https://user-images.githubusercontent.com/23398472/124231131-81a4bb00-db2d-11eb-9d10-8e577ca7b177.gif)
![Textobject tree-sitter demo](https://user-images.githubusercontent.com/23398472/132537398-2a2e0a54-582b-44ab-a77f-eb818942203d.gif)

- `ma` - Select around the object (`va` in Vim, `<alt-a>` in Kakoune)
- `mi` - Select inside the object (`vi` in Vim, `<alt-i>` in Kakoune)
- `mI` - Select inside the object, allowing surrounding spaces across lines

| Key after `mi` or `ma` | Textobject selected      |
| ---                    | ---                      |
| `w`                    | Word                     |
| `W`                    | WORD                     |
| `p`                    | Paragraph                |
| `(`, `[`, `'`, etc.    | Specified surround pairs |
| `m`                    | The closest surround pair    |
| `f`                    | Function                 |
| `t`                    | Type (or Class)          |
| `a`                    | Argument/parameter       |
| `c`                    | Comment                  |
| `T`                    | Test                     |
| `g`                    | Change                   |
| `x`                    | (X)HTML element          |

`mi<Space>` selects between spaces on the cursor's current line. When a space
is missing on either side, the start or end of that line acts as the boundary.
The line ending is excluded, so a line with no spaces selects only its text.
On an empty line, the cursor stays on that line.
Counts select farther spaces but never leave the current line. A cursor on a
space keeps its existing selection, as with other symmetric delimiters.

`mI<Space>` preserves the previous behavior: it searches for surrounding spaces
across line breaks and selects everything between them. Other text objects
behave the same with `mi` and `mI`, including matching quotes and brackets
across lines. `ma<Space>` also keeps its existing behavior across lines.

> 💡 `f`, `t`, etc. need a tree-sitter grammar active for the current
document and a special tree-sitter query file to work properly. [Only
some grammars](./lang-support.md) currently have the query file implemented.
Contributions are welcome!

## Navigating using tree-sitter textobjects

Navigating between functions, classes, parameters, and other elements is
possible using tree-sitter and textobject queries. For
example to move to the next function use `]f`, to move to previous
type use `[t`, and so on.

![Tree-sitter-nav-demo](https://user-images.githubusercontent.com/23398472/152332550-7dfff043-36a2-4aec-b8f2-77c13eb56d6f.gif)

For the full reference see the [unimpaired](./keymap.html#unimpaired) section of the key bind
documentation.

> 💡 This feature relies on tree-sitter textobjects
> and requires the corresponding query file to work properly.

