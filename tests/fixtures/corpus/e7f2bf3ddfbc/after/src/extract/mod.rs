fn literal_symbol_array(node: &Node<'_>) -> Option<Vec<String>> {
    if let Some(array) = node.as_array_node() {
        let symbols: Vec<String> = array
            .elements()
            .iter()
            .filter_map(|element| literal_name(&element))
            .collect();
        return (!symbols.is_empty()).then_some(symbols);
    }
    let call = node.as_call_node()?;
    if !crate::core::IDENTITY.contains(&method_name(&call)?) {
        return None;
    }
    literal_symbol_array(&call.receiver()?)
}
