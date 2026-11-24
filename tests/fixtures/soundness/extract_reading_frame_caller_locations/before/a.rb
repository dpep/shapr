class A
  def go(a)
    puts caller_locations.first.to_s + a.to_s
    puts a.to_s + 'y'
    1
  end
end
