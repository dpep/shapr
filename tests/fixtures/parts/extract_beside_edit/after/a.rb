class Report
  def render(title)
    puts title
    rule
    puts "end"
  end

  def rule
    puts "-" * 10
    puts "=" * 10
  end

  def footer
    puts 2
  end
end
