; XML highlight query (.csproj, .props, .targets, .slnx, .config, .resx, .xaml, …).
; Tags use the same kinds as Razor markup, so both look alike in every theme. MSBuild references
; (`$(Property)`, `@(Item)`, `%(Metadata)`) inside values are colored by `Highlighter::highlight`.

(Comment) @comment

; <?xml version="1.0" encoding="utf-8"?> and other processing instructions
"xml" @keyword
[ "version" "encoding" "standalone" ] @html_attribute_name
[ (VersionNum) (EncName) "yes" "no" ] @html_attribute_value
(PI (PITarget) @keyword)
(doctypedecl "DOCTYPE" @keyword)
(doctypedecl (Name) @html_element_name)

; Tags and attributes
(STag (Name) @html_element_name)
(ETag (Name) @html_element_name)
(EmptyElemTag (Name) @html_element_name)
(Attribute (Name) @html_attribute_name)
(Attribute (AttValue) @html_attribute_value)

; Entities (&amp; &#160;)
[ (EntityRef) (CharRef) ] @string_escape

; <![CDATA[ … ]]>
(CDSect (CDStart) @html_tag_delimiter)
(CDSect "]]>" @html_tag_delimiter)
(CDSect (CData) @string)

[ "<?" "?>" "<!" "<" ">" "</" "/>" ] @html_tag_delimiter
"=" @html_tag_delimiter
