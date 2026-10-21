[
  (block)
  (function_item)
  (closure_expression)
  (for_expression)
  (match_arm)
  (if_expression)
  (while_expression)
] @local.scope

; An upper-case name in a pattern is a unit variant or a constant, not a binding.
(
  [
    (let_declaration pattern: (identifier) @local.definition)
    (let_condition pattern: (identifier) @local.definition)
    (for_expression pattern: (identifier) @local.definition)
    (match_pattern . (identifier) @local.definition)
    (tuple_pattern (identifier) @local.definition)
    (tuple_struct_pattern type: (_) (identifier) @local.definition)
    (slice_pattern (identifier) @local.definition)
    (ref_pattern (identifier) @local.definition)
    (mut_pattern (identifier) @local.definition)
    (reference_pattern (identifier) @local.definition)
    (captured_pattern . (identifier) @local.definition)
    (or_pattern (identifier) @local.definition)
    (field_pattern pattern: (identifier) @local.definition)
    (field_pattern name: (shorthand_field_identifier) @local.definition)
  ]
  (#match? @local.definition "^[a-z_]")
)

[(let_declaration) (let_condition)] @local.declaration

(parameter pattern: (identifier) @local.definition.hoisted)
(closure_parameters (identifier) @local.definition.hoisted)

(block (function_item name: (identifier) @local.definition.hoisted))
(block (const_item name: (identifier) @local.definition.hoisted))
(block (static_item name: (identifier) @local.definition.hoisted))
(block (struct_item name: (type_identifier) @local.definition.hoisted))
(block (enum_item name: (type_identifier) @local.definition.hoisted))
(block (type_item name: (type_identifier) @local.definition.hoisted))

(source_file (function_item name: (identifier) @local.definition.hoisted))
(source_file (const_item name: (identifier) @local.definition.hoisted))
(source_file (static_item name: (identifier) @local.definition.hoisted))
(source_file (struct_item name: (type_identifier) @local.definition.hoisted))
(source_file (enum_item name: (type_identifier) @local.definition.hoisted))
(source_file (union_item name: (type_identifier) @local.definition.hoisted))
(source_file (trait_item name: (type_identifier) @local.definition.hoisted))
(source_file (type_item name: (type_identifier) @local.definition.hoisted))

(identifier) @local.reference
(type_identifier) @local.reference
(scoped_identifier name: (identifier) @local.member)
(scoped_type_identifier name: (type_identifier) @local.member)
