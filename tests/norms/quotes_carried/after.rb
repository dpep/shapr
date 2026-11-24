class Loader
  def a
    x = "one"
    y = "two"
    z = "three"
    silence { load 'lib/x.rb' }
  end
end
