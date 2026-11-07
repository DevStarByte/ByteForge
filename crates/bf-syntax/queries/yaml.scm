; YAML highlight query (Docker Compose files, GitHub workflows, …). Capture names map to theme kinds in
; `src/lib.rs`.

(comment) @comment

[
  (double_quote_scalar)
  (single_quote_scalar)
  (block_scalar)
  (string_scalar)
] @string

(escape_sequence) @string_escape

[
  (integer_scalar)
  (float_scalar)
] @number

[
  (boolean_scalar)
  (null_scalar)
] @keyword

[
  (anchor_name)
  (alias_name)
] @label_name

(tag) @class_name

[
  (yaml_directive)
  (tag_directive)
  (reserved_directive)
] @preprocessor_keyword

(block_mapping_pair
  key: (flow_node [(double_quote_scalar) (single_quote_scalar)] @json_property))

(block_mapping_pair
  key: (flow_node (plain_scalar (string_scalar) @json_property)))

(flow_pair
  key: (flow_node [(double_quote_scalar) (single_quote_scalar)] @json_property))

(flow_pair
  key: (flow_node (plain_scalar (string_scalar) @json_property)))

[
  ","
  "-"
  ":"
  ">"
  "?"
  "|"
  "["
  "]"
  "{"
  "}"
  "*"
  "&"
  "---"
  "..."
] @punctuation
