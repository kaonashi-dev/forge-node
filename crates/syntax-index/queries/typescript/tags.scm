(abstract_class_declaration name: (type_identifier) @name) @definition.class
(interface_declaration name: (type_identifier) @name) @definition.interface
(type_alias_declaration name: (type_identifier) @name) @definition.type
(enum_declaration name: (identifier) @name) @definition.enum
(enum_body name: (property_identifier) @name) @definition.variant
(enum_assignment name: (property_identifier) @name) @definition.variant
(function_signature name: (identifier) @name) @definition.function
(method_signature name: (property_identifier) @name) @definition.method
(abstract_method_signature name: (property_identifier) @name) @definition.method
(property_signature name: (property_identifier) @name) @definition.field
(public_field_definition name: (property_identifier) @name) @definition.field
(module name: (identifier) @name) @definition.module
(internal_module name: (identifier) @name) @definition.module
(ambient_declaration
  (lexical_declaration (variable_declarator name: (identifier) @name) @definition.variable))
(ambient_declaration
  (variable_declaration (variable_declarator name: (identifier) @name) @definition.variable))
