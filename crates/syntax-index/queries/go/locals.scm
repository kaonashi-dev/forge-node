[
  (function_declaration)
  (method_declaration)
  (func_literal)
  (block)
  (for_statement)
  (if_statement)
  (expression_switch_statement)
  (type_switch_statement)
  (expression_case)
  (type_case)
  (default_case)
  (communication_case)
] @local.scope

(parameter_declaration name: (identifier) @local.definition.hoisted)
(variadic_parameter_declaration name: (identifier) @local.definition.hoisted)
(short_var_declaration left: (expression_list (identifier) @local.definition))
(var_spec name: (identifier) @local.definition)
(const_spec name: (identifier) @local.definition)
(range_clause left: (expression_list (identifier) @local.definition))
(type_switch_statement alias: (expression_list (identifier) @local.definition))
(receive_statement left: (expression_list (identifier) @local.definition))
(type_spec name: (type_identifier) @local.definition)
[(short_var_declaration) (var_spec) (const_spec)] @local.declaration

; A package-level name is visible throughout, whatever its line.
(source_file (function_declaration name: (identifier) @local.definition.hoisted))
(source_file (var_declaration (var_spec name: (identifier) @local.definition.hoisted)))
(source_file
  (var_declaration (var_spec_list (var_spec name: (identifier) @local.definition.hoisted))))
(source_file (const_declaration (const_spec name: (identifier) @local.definition.hoisted)))
(source_file (type_declaration (type_spec name: (type_identifier) @local.definition.hoisted)))
(source_file (type_declaration (type_alias name: (type_identifier) @local.definition.hoisted)))

(identifier) @local.reference
(type_identifier) @local.reference
(qualified_type name: (type_identifier) @local.member)
