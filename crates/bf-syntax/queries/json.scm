; JSON highlight query. Capture names map to theme kinds in `src/kinds.rs`.

(string) @json_string
(pair key: (string) @json_property)

(number) @number

[
  (true)
  (false)
  (null)
] @keyword

(comment) @comment

[
  "{"
  "}"
  "["
  "]"
  ","
  ":"
] @punctuation
