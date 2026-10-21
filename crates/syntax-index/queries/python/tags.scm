(class_definition name: (identifier) @name) @definition.class
(function_definition name: (identifier) @name) @definition.function
(module (expression_statement (assignment left: (identifier) @name) @definition.constant))
(module
  (expression_statement
    (assignment left: (pattern_list (identifier) @name)) @definition.constant))
(class_definition
  body: (block (expression_statement (assignment left: (identifier) @name) @definition.field)))
(type_alias_statement left: (type (identifier) @name)) @definition.type
