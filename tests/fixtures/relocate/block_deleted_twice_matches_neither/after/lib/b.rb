class B
  def width(rows)
    keys = rows.first.keys
    names = keys.map(&:to_s)
    width = names.map(&:size).max
    width
  end
end
