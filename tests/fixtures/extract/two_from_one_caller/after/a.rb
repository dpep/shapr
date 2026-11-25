class Report
  def render(title, rows)
    puts title
    print_header(rows)
    print_rows(rows)
    puts "end"
  end

  def print_header(rows)
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
  end

  def print_rows(rows)
    rows.each { |row| puts row.values.join(" | ") }
  end
end
