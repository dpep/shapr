class Report
  def render(rows, width)
    pad = width * 2
    puts format_row(cells(rows, width))
    puts "end"
  end

  def cells(rows, pad)
    rows.map { |r| r.to_s.ljust(pad) }.join(" | ")
  end
end
