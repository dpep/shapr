class Report
  def render(rows)
    measure(rows)
    rows.each do |row, width_hint|
      puts row.ljust(width)
    end
  end

  def measure(rows)
    puts "rows"
    width = rows.map(&:size).max * 2
  end
end
