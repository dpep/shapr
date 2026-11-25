class Report
  def render(rows)
    puts "rows"
    width = rows.map(&:size).max * 2
    rows.each do |row, width_hint|
      puts row.ljust(width)
    end
  end
end
