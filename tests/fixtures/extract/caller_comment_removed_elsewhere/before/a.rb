class Report
  def render(title, rows)
    # the title first
    puts title
    header = rows.first.keys.map(&:to_s)
    puts header.join(" | ")
    puts "-" * header.join(" | ").size
    puts "end"
  end
end
