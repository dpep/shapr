class Report
  def render(title, rows)
    puts title
    print_header(rows)
    rows.each { |row| puts row.values.join(" | ") }
    puts "end"
  end

  def print_header(rows)
    names = rows.flat_map(&:keys).uniq.sort
    widths = names.map { |n| [n.size, rows.map { |r| r[n].to_s.size }.max].max }
    puts names.zip(widths).map { |n, w| n.to_s.ljust(w) }.join(" | ")
    puts "-" * header.join(" | ").size
  end
end
