class A
  def a(rows)
    keys = rows.first.keys
    names = keys.map(&:to_s)
    width = names.map(&:size).max
    width + 1
  end
end
