class Report
  def render(title, rows)
    puts title
    puts "end"
  end

  def summary(rows)
    print_header(rows)
    puts rows.size
  end

  def print_header(rows)
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
  end
end
