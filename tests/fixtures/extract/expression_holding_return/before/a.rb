class Report
  def render(rows, width)
    total = rows.first || return
    puts format_row(total.to_s.ljust(width).strip)
  end
end
