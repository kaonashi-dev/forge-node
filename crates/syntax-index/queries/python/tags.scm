(class_definition name: (identifier) @name) @definition.class
(function_definition name: (identifier) @name) @definition.function
(module (expression_statement (assignment left: (identifier) @name) @definition.constant))
(module
  (expression_statement
    (assignment left: (pattern_list (identifier) @name)) @definition.constant))
(class_definition
  body: (block (expression_statement (assignment left: (identifier) @name) @definition.field)))
(type_alias_statement left: (type (identifier) @name)) @definition.type
(call function: (identifier) @name) @reference.call
(call function: (attribute attribute: (identifier) @name)) @reference.call
(import_statement name: (dotted_name) @name) @reference.import
(import_statement name: (aliased_import name: (dotted_name) @name)) @reference.import
(import_from_statement module_name: (dotted_name) @name) @reference.import
(import_from_statement module_name: (relative_import) @name) @reference.import
(class_definition
  name: (identifier) @name
  superclasses: (argument_list (identifier) @parent)) @reference.implementation
