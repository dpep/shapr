class Report
  def render(title, rows)
    puts title
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
    puts header.size
  end
end
