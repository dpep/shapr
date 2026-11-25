class Report
  def render(title, rows)
    puts title
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
    print_footer(rows)
    rows.each { |row| puts row.values.join(" | ") }
    puts "end"
  end

  def print_footer(rows)
    header = rows.last.keys.map(&:to_s)
    puts header.join(" / ")
    puts "=" * header.join(" / ").size
  end
end
