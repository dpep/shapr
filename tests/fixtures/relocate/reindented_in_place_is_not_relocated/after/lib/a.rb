class A
  def a(rows)
    return 0 if rows.empty?

    begin
      keys = rows.first.keys
      names = keys.map(&:to_s)
      width = names.map(&:size).max
      width + 1
    end
  end
end
