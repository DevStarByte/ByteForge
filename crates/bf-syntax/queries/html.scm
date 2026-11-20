; Added to tree-sitter-html's own query, so HTML files look like the markup in Razor files:
; the whole quoted value (quotes too) and the tag delimiters get the HTML tokens.

(quoted_attribute_value) @html_attribute_value
(attribute_value) @html_attribute_value

[
  "<"
  ">"
  "</"
  "/>"
] @html_tag_delimiter
