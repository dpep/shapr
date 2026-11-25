class Report
  def render(rows, width)
    total = first_row(rows)
    puts format_row(total.to_s.ljust(width).strip)
  end

  def first_row(rows)
    rows.first || return
  end
end
