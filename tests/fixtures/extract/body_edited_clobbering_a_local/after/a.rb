class Report
  def render(title, rows)
    total = 0
    puts title
    add_rows(title, rows)
    puts total
  end

  def add_rows(title, rows)
    return if rows.empty?

    total = rows.sum { |r| r[:amount] } * 2
    puts "-" * title.size
  end
end
