class Report
  def render(rows, width)
    puts format_row(rows.map { |r| r.to_s.ljust(width) }.join(" | "))
    puts "end"
  end
end
