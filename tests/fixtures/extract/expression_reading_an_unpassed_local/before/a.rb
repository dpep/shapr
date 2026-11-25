class Report
  def render(rows, width)
    pad = width * 2
    puts format_row(rows.map { |r| r.to_s.ljust(pad) }.join(" | "))
    puts "end"
  end
end
