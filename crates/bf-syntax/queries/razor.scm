; Razor additions, appended to the C# query (`csharp.scm`) for `.razor` and `.cshtml` files.
;
; The Razor grammar embeds C# with tree-sitter-c-sharp's node names, so every C# pattern
; applies inside code blocks and expressions. HTML inside `element` nodes has no child nodes
; for tags and attributes; `Highlighter::highlight` colors that markup with a small lexer.

[
  (razor_comment)
  (html_comment)
] @comment

(html_doctype) @html_tag_delimiter

; Directives (@page, @using, @inject, @code, @section, …).
[
  "at_page"
  "at_using"
  "at_model"
  "at_rendermode"
  "at_inject"
  "at_implements"
  "at_layout"
  "at_inherits"
  "at_attribute"
  "at_typeparam"
  "at_namespace"
  "at_preservewhitespace"
  "at_block"
  "at_addtaghelper"
  "at_removetaghelper"
  "at_taghelperprefix"
  "at_lock"
  "at_section"
] @razor_directive

; Control flow (@if, @foreach, …).
[
  "at_if"
  "at_switch"
  "at_for"
  "at_foreach"
  "at_while"
  "at_do"
  "at_try"
] @keyword_control

(razor_else "else" @keyword_control)
(razor_catch "catch" @keyword_control)
(razor_finally "finally" @keyword_control)

; Directive attributes (@onclick, @bind-Value, …).
(razor_attribute_name) @razor_directive
(razor_rendermode) @property_name

; The transition character always wins over whatever contains it.
"@" @razor_transition
