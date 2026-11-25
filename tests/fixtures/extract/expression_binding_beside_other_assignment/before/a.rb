class Lookup
  def translate(key, values)
    table, column = key.to_s.split(".", 2)
    if column
      model = table == "users" ? self : associated(table)
      return model.translate(values)
    end

    model = key.to_s == primary ? self : referenced(key)
    model.translate(values) if model
  end
end
