class Report
  def render(title, rows)
    puts title.upcase
    print_header(rows)
    rows.each { |row| puts row.values.join(" | ") }
    puts "end"
  end

  def print_header(rows)
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
  end
end
