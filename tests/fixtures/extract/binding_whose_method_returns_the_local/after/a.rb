class Report
  def render(title, rows)
    puts title
    total = sum_rows(rows)
    puts total
  end

  def sum_rows(rows)
    total = rows.map(&:amount).sum * 2 + rows.size
    total
  end
end
