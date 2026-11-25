class Report
  def render(rows, width)
    puts format_row(cells(rows, width))
    puts "end"
  end

  def cells(rows, width)
    rows.map { |r| r.to_s.ljust(width) }.join(" | ")
  end
end
