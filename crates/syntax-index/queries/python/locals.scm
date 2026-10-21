[
  (function_definition)
  (lambda)
  (list_comprehension)
  (set_comprehension)
  (dictionary_comprehension)
  (generator_expression)
] @local.scope

; A class body's names are not visible inside its methods.
(class_definition) @local.scope.isolated

; An assignment anywhere in a function makes the name local to all of it.
(parameters (identifier) @local.definition.hoisted)
(lambda_parameters (identifier) @local.definition.hoisted)
(default_parameter name: (identifier) @local.definition.hoisted)
(typed_parameter (identifier) @local.definition.hoisted)
(typed_default_parameter name: (identifier) @local.definition.hoisted)
(list_splat_pattern (identifier) @local.definition.hoisted)
(dictionary_splat_pattern (identifier) @local.definition.hoisted)
(assignment left: (identifier) @local.definition.hoisted)
(assignment left: (pattern_list (identifier) @local.definition.hoisted))
(assignment left: (tuple_pattern (identifier) @local.definition.hoisted))
(augmented_assignment left: (identifier) @local.definition.hoisted)
(for_statement left: (identifier) @local.definition.hoisted)
(for_statement left: (pattern_list (identifier) @local.definition.hoisted))
(for_in_clause left: (identifier) @local.definition.hoisted)
(for_in_clause left: (pattern_list (identifier) @local.definition.hoisted))
(as_pattern_target (identifier) @local.definition.hoisted)
(named_expression name: (identifier) @local.definition.hoisted)
(function_definition name: (identifier) @local.definition.hoisted)
(class_definition name: (identifier) @local.definition.hoisted)

(identifier) @local.reference
(attribute attribute: (identifier) @local.member)
(keyword_argument name: (identifier) @local.member)
