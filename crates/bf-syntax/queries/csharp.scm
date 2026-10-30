; C# highlight query.
;
; Capture names map to Roslyn classification names in `src/kinds.rs`.
; Resolution rules (see `Highlighter::highlight`): the innermost capture wins,
; and for captures with an identical range the pattern that appears LATER in
; this file wins. So: generic fallbacks first, specific contexts last.

; ---------------------------------------------------------------------------
; Fallback
; ---------------------------------------------------------------------------

(identifier) @identifier

; ---------------------------------------------------------------------------
; Type references (heuristic: capitalized -> class, I + uppercase -> interface,
; T / T + uppercase -> type parameter)
; ---------------------------------------------------------------------------

((_ type: (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((_ type: (qualified_name name: (identifier) @class_name)) (#match? @class_name "^[A-Z]"))
((method_declaration returns: (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((method_declaration returns: (qualified_name name: (identifier) @class_name)) (#match? @class_name "^[A-Z]"))
((generic_name (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((type_argument_list (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((type_argument_list (qualified_name name: (identifier) @class_name)) (#match? @class_name "^[A-Z]"))
((base_list (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((base_list (qualified_name name: (identifier) @class_name)) (#match? @class_name "^[A-Z]"))
((primary_constructor_base_type type: (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((as_expression right: (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((is_expression right: (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((type_pattern type: (identifier) @class_name) (#match? @class_name "^[A-Z]"))
((attribute name: (identifier) @class_name))
((attribute name: (qualified_name name: (identifier) @class_name)))

; Interface heuristic for everything matched above.
((_ type: (identifier) @interface_name) (#match? @interface_name "^I[A-Z]"))
((_ type: (qualified_name name: (identifier) @interface_name)) (#match? @interface_name "^I[A-Z]"))
((method_declaration returns: (identifier) @interface_name) (#match? @interface_name "^I[A-Z]"))
((generic_name (identifier) @interface_name) (#match? @interface_name "^I[A-Z]"))
((type_argument_list (identifier) @interface_name) (#match? @interface_name "^I[A-Z]"))
((base_list (identifier) @interface_name) (#match? @interface_name "^I[A-Z]"))
((base_list (qualified_name name: (identifier) @interface_name)) (#match? @interface_name "^I[A-Z]"))

; Type-parameter heuristic for type references.
((_ type: (identifier) @type_parameter_name) (#match? @type_parameter_name "^T([A-Z][A-Za-z0-9]*)?$"))
((method_declaration returns: (identifier) @type_parameter_name) (#match? @type_parameter_name "^T([A-Z][A-Za-z0-9]*)?$"))
((type_argument_list (identifier) @type_parameter_name) (#match? @type_parameter_name "^T([A-Z][A-Za-z0-9]*)?$"))

; ---------------------------------------------------------------------------
; Namespaces
; ---------------------------------------------------------------------------

(qualified_name qualifier: (identifier) @namespace_name)
(using_directive (identifier) @namespace_name)
(using_directive (qualified_name name: (identifier) @namespace_name))
(namespace_declaration name: (identifier) @namespace_name)
(namespace_declaration name: (qualified_name name: (identifier) @namespace_name))
(file_scoped_namespace_declaration name: (identifier) @namespace_name)
(file_scoped_namespace_declaration name: (qualified_name name: (identifier) @namespace_name))

; ---------------------------------------------------------------------------
; Member access (heuristic: capitalized member -> property)
; ---------------------------------------------------------------------------

((member_access_expression name: (identifier) @property_name) (#match? @property_name "^[A-Z]"))

; ---------------------------------------------------------------------------
; Declarations
; ---------------------------------------------------------------------------

(class_declaration name: (identifier) @class_name)
(record_declaration name: (identifier) @record_class_name)
(record_declaration "struct" name: (identifier) @struct_name)
(struct_declaration name: (identifier) @struct_name)
(interface_declaration name: (identifier) @interface_name)
(enum_declaration name: (identifier) @enum_name)
(delegate_declaration name: (identifier) @delegate_name)
(constructor_declaration name: (identifier) @class_name)
(destructor_declaration name: (identifier) @class_name)

(type_parameter name: (identifier) @type_parameter_name)
(type_parameter_constraints_clause (identifier) @type_parameter_name)

(method_declaration name: (identifier) @method_name)
(local_function_statement name: (identifier) @method_name)

(property_declaration name: (identifier) @property_name)

(field_declaration
  (variable_declaration (variable_declarator name: (identifier) @field_name)))
(field_declaration
  (modifier "const")
  (variable_declaration (variable_declarator name: (identifier) @constant_name)))

(event_declaration name: (identifier) @event_name)
(event_field_declaration
  (variable_declaration (variable_declarator name: (identifier) @event_name)))

(enum_member_declaration name: (identifier) @enum_member_name)

(parameter name: (identifier) @parameter_name)
(lambda_expression parameters: (implicit_parameter) @parameter_name)

(local_declaration_statement
  (variable_declaration (variable_declarator name: (identifier) @local_name)))
(local_declaration_statement
  (modifier "const")
  (variable_declaration (variable_declarator name: (identifier) @constant_name)))
(for_statement
  (variable_declaration (variable_declarator name: (identifier) @local_name)))
(using_statement
  (variable_declaration (variable_declarator name: (identifier) @local_name)))
(fixed_statement
  (variable_declaration (variable_declarator name: (identifier) @local_name)))
(foreach_statement left: (identifier) @local_name)
(declaration_expression name: (identifier) @local_name)
(catch_declaration name: (identifier) @local_name)
(declaration_pattern name: (identifier) @local_name)
(var_pattern (identifier) @local_name)
(from_clause (identifier) @local_name)
(let_clause (identifier) @local_name)

(labeled_statement (identifier) @label_name)
(goto_statement (identifier) @label_name)

; ---------------------------------------------------------------------------
; Invocations
; ---------------------------------------------------------------------------

(invocation_expression function: (identifier) @method_name)
(invocation_expression function: (generic_name (identifier) @method_name))
(invocation_expression
  function: (member_access_expression name: (identifier) @method_name))
(invocation_expression
  function: (member_access_expression name: (generic_name (identifier) @method_name)))
(invocation_expression
  function: (conditional_access_expression
    (member_binding_expression name: (identifier) @method_name)))
(invocation_expression
  function: (conditional_access_expression
    (member_binding_expression name: (generic_name (identifier) @method_name))))

; `nameof(x)` is a keyword-like construct.
((invocation_expression function: (identifier) @keyword)
  (#eq? @keyword "nameof"))

; ---------------------------------------------------------------------------
; Literals
; ---------------------------------------------------------------------------

[
  (integer_literal)
  (real_literal)
] @number

[
  (string_literal)
  (character_literal)
  (raw_string_literal)
] @string

(verbatim_string_literal) @string_verbatim

(interpolation_start) @string
((interpolation_start) @string_verbatim (#match? @string_verbatim "@"))

(interpolated_string_expression
  ["\"" (interpolation_quote) (string_content)] @string)

(interpolated_string_expression
  (interpolation_start) @_start
  (#match? @_start "@")
  ["\"" (interpolation_quote) (string_content)] @string_verbatim)

(escape_sequence) @string_escape

[
  (boolean_literal)
  (null_literal)
] @keyword

; ---------------------------------------------------------------------------
; Comments
; ---------------------------------------------------------------------------

(comment) @comment
((comment) @xml_doc_comment (#match? @xml_doc_comment "^///"))

; ---------------------------------------------------------------------------
; Preprocessor
; ---------------------------------------------------------------------------

[
  "#if"
  "#elif"
  "#else"
  "#endif"
  "#region"
  "#endregion"
  "#define"
  "#undef"
  "#error"
  "#warning"
  "#line"
  "#nullable"
  "#pragma"
] @preprocessor_keyword

(preproc_pragma ["warning" "disable" "restore" "checksum"] @preprocessor_keyword)
(preproc_nullable ["enable" "disable" "restore" "annotations" "warnings"] @preprocessor_keyword)

; ---------------------------------------------------------------------------
; Operators & punctuation
; ---------------------------------------------------------------------------

[
  "!" "!=" "%" "%=" "&" "&&" "&=" "*" "*=" "+" "++" "+=" "-" "--" "-=" "->"
  "/" "/=" ":" "::" "<" "<<" "<<=" "<=" "=" "==" "=>" ">" ">=" ">>" ">>="
  ">>>" ">>>=" "?" "??" "??=" "^" "^=" "|" "|=" "||" "~" ".."
] @operator

[
  ";" "." "," "(" ")" "[" "]" "{" "}"
  (interpolation_brace)
] @punctuation

(type_argument_list ["<" ">"] @punctuation)
(type_parameter_list ["<" ">"] @punctuation)

; ---------------------------------------------------------------------------
; Keywords
; ---------------------------------------------------------------------------

[
  (predefined_type)
  (implicit_type)
  (modifier)
] @keyword

[
  "abstract" "add" "alias" "and" "as" "ascending" "async" "base" "by" "checked"
  "class" "const" "delegate" "descending" "enum" "equals" "event" "explicit"
  "extern" "file" "fixed" "from" "get" "global" "group" "implicit" "in" "init"
  "interface" "internal" "into" "is" "join" "let" "lock" "managed" "namespace"
  "new" "not" "notnull" "on" "operator" "or" "orderby" "out" "override" "params"
  "partial" "private" "protected" "public" "readonly" "record" "ref" "remove"
  "required" "scoped" "sealed" "select" "set" "sizeof" "stackalloc" "static"
  "struct" "this" "typeof" "unchecked" "unmanaged" "unsafe" "using" "var"
  "virtual" "volatile" "when" "where" "with" "__makeref" "__reftype" "__refvalue"
] @keyword

[
  "if" "else" "for" "foreach" "while" "do" "switch" "case" "default" "break"
  "continue" "return" "goto" "throw" "try" "catch" "finally" "yield" "await"
] @keyword_control

; `default(T)` / `default` literal is an expression keyword, not control flow.
(default_expression "default" @keyword)
