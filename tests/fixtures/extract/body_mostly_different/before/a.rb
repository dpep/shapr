class Report
  def render(title, rows)
    puts title
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
    rows.each { |row| puts row.values.join(" | ") }
    puts "end"
  end
end
