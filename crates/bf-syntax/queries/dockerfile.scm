; Dockerfile / Containerfile highlight query. Capture names map to theme kinds in `src/lib.rs`.

[
  "FROM"
  "AS"
  "RUN"
  "CMD"
  "LABEL"
  "EXPOSE"
  "ENV"
  "ADD"
  "COPY"
  "ENTRYPOINT"
  "VOLUME"
  "USER"
  "WORKDIR"
  "ARG"
  "ONBUILD"
  "STOPSIGNAL"
  "HEALTHCHECK"
  "SHELL"
  "MAINTAINER"
  "CROSS_BUILD"
] @keyword

(comment) @comment

; Base images and build stages.
(image_spec name: (image_name) @class_name)
(image_tag) @number
(image_digest) @number
(image_alias) @label_name

[
  (double_quoted_string)
  (single_quoted_string)
  (json_string)
  (heredoc_block)
] @string

[
  (heredoc_marker)
  (heredoc_end)
] @label_name

(escape_sequence) @string_escape

; `$VAR` and `${VAR:-default}`.
(expansion) @local_name
(expansion_operator) @operator

(arg_pair name: (unquoted_string) @property_name)
(env_pair name: (unquoted_string) @property_name)
(label_pair key: (_) @property_name)

; `--from=build`, `--mount=type=cache,…`.
(param) @parameter_name
(mount_param) @parameter_name

(expose_port) @number

[
  "["
  "]"
  ","
] @punctuation
