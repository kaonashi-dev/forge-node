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

; The keys of a module's constant objects, one level of nesting deep, are
; what `UI_STRINGS.title` names. A key in any other object literal (a call's
; options, a return value) is data, not a declaration.
(program
  (lexical_declaration
    (variable_declarator
      value: (object (pair key: (property_identifier) @name) @definition.field))))
(program
  (export_statement
    declaration: (lexical_declaration
      (variable_declarator
        value: (object (pair key: (property_identifier) @name) @definition.field)))))
(program
  (lexical_declaration
    (variable_declarator
      value: (object
        (pair value: (object (pair key: (property_identifier) @name) @definition.field))))))
(program
  (export_statement
    declaration: (lexical_declaration
      (variable_declarator
        value: (object
          (pair value: (object (pair key: (property_identifier) @name) @definition.field)))))))
