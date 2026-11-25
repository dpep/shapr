class Report
  def render(title, rows)
    puts title
    header = rows.first.keys.map(&:to_s)
    return if header.empty?
    puts header.join(" | ")
    puts "end"
  end
end
