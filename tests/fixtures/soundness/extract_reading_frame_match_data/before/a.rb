class A
  def go(a)
    puts $~.to_s + a.to_s + 'zz'
    puts a.to_s * 2
    1
  end
end
