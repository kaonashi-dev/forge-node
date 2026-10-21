[
  (statement_block)
  (function_declaration)
  (generator_function_declaration)
  (function_expression)
  (arrow_function)
  (method_definition)
  (for_statement)
  (for_in_statement)
  (catch_clause)
] @local.scope

; `let` and `const` are visible (if unusable) before their line, so every
; binding counts for its whole scope.
(variable_declarator name: (identifier) @local.definition.hoisted)
(object_pattern (shorthand_property_identifier_pattern) @local.definition.hoisted)
(object_assignment_pattern left: (shorthand_property_identifier_pattern) @local.definition.hoisted)
(pair_pattern value: (identifier) @local.definition.hoisted)
(array_pattern (identifier) @local.definition.hoisted)
(assignment_pattern left: (identifier) @local.definition.hoisted)
(rest_pattern (identifier) @local.definition.hoisted)
(function_declaration name: (identifier) @local.definition.hoisted)
(generator_function_declaration name: (identifier) @local.definition.hoisted)
(class_declaration name: (_) @local.definition.hoisted)
(for_in_statement left: (identifier) @local.definition.hoisted)
(catch_clause parameter: (identifier) @local.definition.hoisted)
(arrow_function parameter: (identifier) @local.definition.hoisted)

(identifier) @local.reference
(shorthand_property_identifier) @local.reference
