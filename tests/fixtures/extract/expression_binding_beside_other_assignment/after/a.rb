class Lookup
  def translate(key, values)
    table, column = key.to_s.split(".", 2)
    if column
      model = table == "users" ? self : associated(table)
      return model.translate(values)
    end

    model = key_target(key)
    model.translate(values) if model
  end

  def key_target(key)
    key.to_s == primary ? self : referenced(key)
  end
end
