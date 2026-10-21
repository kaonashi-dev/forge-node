(function_declaration name: (identifier) @name) @definition.function
(generator_function_declaration name: (identifier) @name) @definition.function
(class_declaration name: (_) @name) @definition.class
(method_definition name: (property_identifier) @name) @definition.method

(program (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable))
(program (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable))
(program
  (export_statement
    declaration: (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable)))
(program
  (export_statement
    declaration: (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable)))

(variable_declarator
  name: (identifier) @name
  value: [(arrow_function) (function_expression)]) @definition.function
(assignment_expression
  left: (member_expression property: (property_identifier) @name)
  right: [(arrow_function) (function_expression)]) @definition.function
(pair
  key: (property_identifier) @name
  value: [(arrow_function) (function_expression)]) @definition.function
