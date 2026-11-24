class A
  def go(a)
    puts caller.first.to_s + a.to_s
    puts a.to_s + 'y' + 'z'
    1
  end
end
