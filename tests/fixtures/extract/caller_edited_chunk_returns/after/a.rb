class Report
  def render(title, rows)
    puts title.upcase
    print_header(rows)
    puts "end"
  end

  def print_header(rows)
    header = rows.first.keys.map(&:to_s)
    return if header.empty?
    puts header.join(" | ")
  end
end
