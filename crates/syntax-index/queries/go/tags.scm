(function_declaration name: (identifier) @name) @definition.function
(method_declaration name: (field_identifier) @name) @definition.method
(type_spec name: (type_identifier) @name) @definition.type
(type_alias name: (type_identifier) @name) @definition.type
(method_elem name: (field_identifier) @name) @definition.method
(field_declaration name: (field_identifier) @name) @definition.field
(source_file (var_declaration (var_spec name: (identifier) @name) @definition.variable))
(source_file
  (var_declaration (var_spec_list (var_spec name: (identifier) @name) @definition.variable)))
(source_file (const_declaration (const_spec name: (identifier) @name) @definition.constant))
(call_expression function: (identifier) @name) @reference.call
(call_expression function: (selector_expression field: (field_identifier) @name)) @reference.call
(import_spec path: (interpreted_string_literal (interpreted_string_literal_content) @name)) @reference.import
